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
//! # What S2D adds (MK7 S2, the container slice)
//!
//! `List<T>`, `Map<K, V>`, `Box<T>` and `[T; N]`. MK7 §3.2 names the one
//! exception to "the runtime stays C": the monomorphized container code, which
//! `codegen_c.rs` generates per element or value type and which therefore cannot
//! live in a fixed C file. So it is generated into the module, as IR, one
//! function set per instantiation, with the same function names, the same bodies
//! and the same panics the C backend writes:
//!
//! - `List<T>` is `{ data, len, cap }`; `reserve` doubles through `realloc`, and
//!   `get`/`set` run the same bounds check (`List.get: index out of bounds`). The
//!   S1 methods (`sort`, `reverse`, `contains`, `slice`, `clone`) are emitted
//!   with the same element gates `types::value_is_sortable` and its siblings
//!   decide, so a `datac` element gets `sort` but never `contains`.
//! - `Box<T>` is one pointer, `empty`/`of`/`get`/`set`/`take`, and `take` moves
//!   the value out, which is what makes a recursive `datac` expressible.
//! - `Map<K, V>` keeps its entries dense in insertion order and a separate
//!   open-addressed index of `entry index + 1` (`0` empty, `SIZE_MAX`
//!   tombstone), so `keys()` returns insertion order. Its key helpers are the
//!   runtime's own (`kelp_map_hash`, `kelp_map_mix`, `kelp_map_key_eq`),
//!   reached by declaration rather than re-derived.
//! - `[T; N]` is inline, not a pointer: `len()` is the type's constant,
//!   `get`/`set` are the same bounds check with the array's panic text, and the
//!   repeat literal goes through a generated `fill` loop rather than N
//!   initializers.
//!
//! A container header is an aggregate over 16 bytes, so it never crosses to the
//! C runtime by value: every helper the module declares takes a pointer
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
//!   than asked for a value it does not have. `codegen_c.rs::write_function_body`
//!   draws the same line, and without it the backend reported its own
//!   "internal error" for `move_param_return` and `panic_tail`.
//!
//! `str` and `View` are pointer-plus-length and are untouched: a `&str` is the
//! pointer itself, and a borrow of a heap type stays the address the runtime
//! helpers take.
//!
//! # What S2E adds (MK7 S2, the closure slice)
//!
//! Function values and closures, mirrored from `codegen_c.rs`: a declared `fun`
//! named as a value, a `fun` literal (with and without captures), a call through
//! a value, and a closure or a function value as a local, a parameter, a return
//! value, a `datac` field and a `Box` element.
//!
//! - a `Ty::Fn` is a bare `ptr`: the address of the declared function, which is
//!   what `FnRef` yields (`LLVM` already treats a function as a pointer value).
//!   It is `Copy`, owns nothing and has no drop.
//! - a `Ty::Closure` is the C backend's fat pointer, one identified struct per
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
//!   the C backend's two shapes: a *known* function (`FnRef`) gets a generated
//!   shim with a `null` environment and allocates nothing, while a value whose
//!   identity only the call knows gets a one-field heap environment and a
//!   thunk that loads the code pointer back out of it.
//! - the per-signature closure drop is the one function in a closure's set, as
//!   it is in the C backend: `if (env != null) { drop(env); env = null; }`.
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
//!   signature its C body has, not the written one: an owned parameter is a
//!   borrow (so it is a pointer), and a `Bytes`/`String` return is 24 bytes, so
//!   it comes back through the hidden first argument clang compiles the body's
//!   `return` into (`declare_extern`, BUG-vN4tR).
//! - a call to one is lowered as a call through the same table the runtime
//!   helpers use: the declaration and the call are the same IR shape whether the
//!   callee's text is in `kelp_rt.c` or in the program's own object.
//! - that object is built per program from `<out>.externs.c`, which holds the
//!   bodies `codegen_c.rs::write_extern` writes, the capability glue, and the
//!   `std/rt` prototypes `runtime_prototypes` writes, so the C text is the C
//!   backend's own (see `program_c_source` for why it cannot be a fixed object
//!   like `kelp_rt.o`).
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
//!   backend emits. `cap_plan` mirrors `codegen_c.rs::Gen::cap_plan` site for
//!   site: the same domain, the same resource arguments, the same pattern
//!   count, the same `.kp` call site. Every checked argument is evaluated
//!   **once** into a value the check and the call share (BUG-Kf7Rn), and the
//!   check is one `kelp_cap_check_path`/`_name`/`_host`/`_hostport`/`_port`
//!   call against the module's compiled-in array.
//! - that array is C data in the per-program object, escaped by the C backend's
//!   own `c_string_literal`, and the check functions are the C backend's own
//!   `runtime/cap_glue.c` with `#define static` around it, so the deny path's
//!   text is the same bytes in both backends. The matcher and the path
//!   normalizer stay Kelp (`std/rt`) and are lowered like any other code.
//!
//! The whole fixture corpus now lowers: `Backend::DEFAULT` is `llvm` since MK7
//! S7, and the `llvm` backend builds every one of the 223 corpus targets the C
//! backend builds, with an empty refusal ranking (MK7 §4.1), and the
//! differential sweep records no disagreement between the two. The differential
//! fuzzer (`kelpc/tests/differential_fuzz.rs`, MK7 S5) covers the surface the
//! curated corpus does not, and found the two shapes the sweep's fixtures were
//! missing.
//!
//! What remains in this file is **refused by name** with a clean diagnostic,
//! never miscompiled and never a panic, for a shape the frontend does not
//! currently produce: MK7 §2 keeps the C backend as the project's own oracle,
//! so a refusal names the construct and says the shortfall is the backend's. A
//! wrong lowering is worse than a refusal, which is why the guards stay.
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
//! # What S4 adds (MK7 S4, the debug locations)
//!
//! DWARF, so that a fault in an `llvm`-built binary names a `.kp` file and line.
//! MK7 §4.4 states this as an exit criterion rather than a nice-to-have: a crash
//! report that points into generated IR instead of a Kelp source line is the
//! difference between actionable and not, and the C backend buys the same
//! resolution with one `#line` per statement (`codegen_c.rs::write_line_marker`).
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
//!   `codegen_c.rs::write_stmt` writes its `#line` from, and the line and column
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
use std::path::Path;

use crate::ast::{BinOp, UnOp};
use crate::borrowck::{DropPlan, DropTarget, Projection};
use crate::llvm_ffi::{
    self, BasicBlock, Builder, Context, DataLayout, DebugBuilder, FloatBinOp, FnType,
    Function as LlvmFunction, IntBinOp, IntCmp, Metadata, Module, RealCmp, TargetMachine, Type,
    Value,
};
use crate::diag::SourceMap;
use crate::lexer::Span;
use crate::opt::OptLevel;
use crate::tir::{Block, Expr, ExprKind, Function, Pd, Program, Stmt, TemplatePiece};
use crate::types::{
    box_c_type, capture_field, clone_fn_name, closure_c_type, closure_mangle, cmp_fn_name,
    drop_fn_name, list_c_type, list_clonable_elem, map_c_type, needs_drop, owned_types, struct_tag,
    ty_mangle, Ty,
};

/// Build the whole program, verify it, write the textual IR beside the binary,
/// run the level's pass pipeline, emit the object and link it with `cc`.
///
/// The artifact contract mirrors the C path's `<out>.c`: this writes `<out>.ll`
/// (MK7 §4.4) so the IR is inspectable next to the binary, and keeps the object
/// it hands `cc` as `<out>.o`. Linking itself is `driver::link_llvm_object`, the
/// sibling of `driver::compile_c`.
///
/// `opt` is the S6 optimization level (MK7 §4.3). It is one level for two
/// things: the `default<O<n>>` pass pipeline `run_passes` runs over the module,
/// and the codegen level the `TargetMachine` is created with. The textual IR is
/// written **before** the pipeline so the artifact beside the binary is the
/// lowering at every level, which is what the MK7 stage gates assert on; the
/// object is what the pipeline and the codegen level produced.
pub fn compile(
    program: &Program,
    drops: &DropPlan,
    sources: &SourceMap,
    out_path: &Path,
    opt: OptLevel,
) -> Result<(), String> {
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
    // (`declare_aggregates`). It is the same machine the object is written with
    // and the one the pass pipeline reads its target from, so the layout the
    // types were built for and the layout codegen assumes cannot be two
    // different ones. It carries the level's codegen half; the pipeline below
    // carries the module half.
    let machine = TargetMachine::for_host_at(opt.codegen_level())?;
    let layout = machine.data_layout();
    // The module has to carry the same data layout the machine lays the object
    // out with before any pass runs: a pass that folds a struct field to a byte
    // offset uses the module's layout, and LLVM's default is not the host's (it
    // aligns an `i64` to 4). See `Module::set_data_layout`.
    module.set_data_layout(&layout);

    let mut lowering = Gen::new(
        &ctx,
        &module,
        program,
        drops,
        sources,
        &layout,
        opt.is_optimized(),
    )?;
    lowering.declare()?;
    // The generated per-`datac` and per-`enum` drops, clones and comparators are
    // defined before the function bodies that call them, mirroring the order
    // `codegen_c.rs` writes each typedef and its drop routine in. The container
    // function sets come next, because a container's own drop calls the
    // element's (MK7 §3.2: one monomorphized set per instantiation, generated
    // into the module).
    lowering.emit_drop_fn_bodies()?;
    lowering.emit_clone_cmp_bodies()?;
    lowering.emit_container_bodies()?;
    // The S2E machinery (per-signature closure drops, the widening's thunk and
    // destructor, the shims) comes before the function bodies, the same order
    // the C backend writes them in.
    lowering.emit_closure_bodies()?;
    // The `pd` globals' two generated functions (MK7 §5), whose bodies call
    // the drops and container sets above.
    lowering.emit_pd_globals()?;
    lowering.emit_all()?;

    // Every location has been attached now, so the debug descriptors are
    // constructed and frozen before the module is written (MK7 S4, §4.4).
    lowering.finalize_debug();

    // A caller must verify before emitting: LLVM's object writer assumes a
    // verified module and can crash on an unverified one rather than fail.
    module.verify()?;

    // The lowering is written before the pipeline runs, so the artifact beside
    // the binary is the IR this backend emitted at every level. The stage gates
    // read this text and assert an instruction's shape; an optimized module
    // would answer a different question.
    let ir_path = out_path.with_extension("ll");
    std::fs::write(&ir_path, module.to_ir_string()).map_err(|e| {
        format!("could not write generated IR to '{}': {e}", ir_path.display())
    })?;

    // The level's module half: LLVM's own `default<O<n>>` pipeline. It is run
    // over the module the machine below will write, with the machine for the
    // target-specific passes. At `-O0` it is `default<O0>`, which adds no pass,
    // so the IR the lowering wrote is the IR the target machine lowers.
    module.run_passes(opt.pipeline(), &machine)?;

    // A pass can leave a module LLVM's own verifier rejects, and the object
    // writer assumes a verified module, so the check runs again after the
    // pipeline rather than only before it.
    module.verify()?;

    let object_path = out_path.with_extension("o");
    machine.emit_object(&module, &object_path)?;

    // The `extern fun` bodies this program reaches and the capability check's
    // own C half, compiled into their own object (MK7 §3.2, §5). Keeping the C
    // source beside the IR is the same artifact contract the C backend's
    // `<out>.c` has: the text an author can read is the text that was compiled.
    let program_object = match program_c_source(program) {
        Some(source) => {
            let c_path = out_path.with_extension("externs.c");
            let obj_path = out_path.with_extension("externs.o");
            crate::driver::compile_c_object(&source, &c_path, &obj_path, opt)
                .map_err(|e| e.message)?;
            Some(obj_path)
        }
        None => None,
    };

    crate::driver::link_llvm_object(&object_path, program_object.as_deref(), out_path, opt)
        .map_err(|e| e.message)
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

/// One gated call site's check plan, the mirror of what
/// `codegen_c.rs::Gen::cap_plan` returns: the domain to match against, how many
/// patterns the calling module holds for it, the call site as written, and
/// which arguments are resource names.
struct CapPlan {
    domain: String,
    pattern_count: usize,
    at: String,
    checks: Vec<crate::stdlib::CapCheck>,
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
    /// The runtime helpers this slice reaches, keyed by their mangled symbol,
    /// plus every `extern fun` the program carries (S3). `kelp_rt.c` defines the
    /// helpers (compiled into the object `build.rs` leaves in `OUT_DIR`) and the
    /// program's own object defines the extern bodies (`externs.rs`), and in both
    /// cases the module only *declares* the function: the C backend emits the
    /// same text into one translation unit.
    runtime: HashMap<String, RtFn<'a>>,
    /// The generated per-`datac` drop functions (`kelp_drop_<name>`), the mirror
    /// of the ones `codegen_c.rs` emits beside each typedef.
    drop_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The `datac`/`enum` names that own memory, so `types::needs_drop` can
    /// answer for a type without re-walking the declarations.
    owned: HashSet<String>,
    /// The LLVM type of every container instantiation the program uses, keyed
    /// by the `tir` container type. A container is a named struct built here
    /// rather than in `llvm_type`, because it is created once and then resolved
    /// by name (`Box<List<i32>>` names another instantiation).
    containers: HashMap<Ty, Type<'a>>,
    /// The entry struct of each `Map<K, V>` instantiation, keyed by the map
    /// type: `{ key, value*, live }`, the shape `write_map`'s typedef has.
    map_entries: HashMap<Ty, Type<'a>>,
    /// The monomorphized container functions, keyed by their mangled symbol
    /// (`kelp_List__i32__push`), with the signature they are declared under.
    container_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The generated `kelp_clone_<name>` functions of a `datac`/`enum` whose
    /// value can be deep-copied, the mirror of the ones `write_struct`/
    /// `write_enum` emit beside each typedef.
    clone_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The S2E closure machinery, keyed by its mangled symbol: the
    /// per-signature drop, the per-signature widening thunk and its
    /// environment's destructor, and the per-`(function, signature)` shim. One
    /// map because `emit_closure_bodies` defines each under the name it was
    /// declared with, and the two cannot drift.
    closure_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The LLVM environment struct of each capturing literal, keyed by the
    /// literal's `tir` key: one field per capture, in capture order, the same
    /// fields `write_env` gives the C struct.
    env_types: HashMap<String, Type<'a>>,
    /// The one-field environment a widened function value's thunk reads its code
    /// pointer out of: `{ ptr f }`, the shape `write_coerce_env` declares.
    /// Shared by every signature because opaque pointers make the field's type
    /// the same for all of them.
    coerce_env_ty: Type<'a>,
    /// The generated `kelp_cmp_<name>` functions of an orderable `datac`/`enum`.
    cmp_fns: HashMap<String, (LlvmFunction<'a>, FnType<'a>)>,
    /// The libc primitives the container function sets call: the standard
    /// headers' allocation, copy and scan functions, declared and never
    /// re-implemented, exactly as the C backend reaches them.
    libc: HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)>,
    /// Every container instantiation the program mentions, so the module emits
    /// exactly the sets it uses (mirrors `codegen_c::Instantiations`).
    instantiations: Instantiations,
    /// The user types whose value can be deep-copied, the same fixpoint
    /// `codegen_c.rs` computes for `List.clone`/`List.slice`.
    clonable: HashSet<String>,
    /// The user types whose value has a derived order, for `List.sort`.
    sortable: HashSet<String>,
    /// One module-scope global per `pd` (MK7 §3.1 and §5), keyed by the
    /// internal `pd#<name>` identifier `tir` names it with, plus that
    /// passdown's own type. The C backend's `static <type> <name>;` is the
    /// same storage: a `pd` initializer fills it once and a function with no
    /// hidden parameter of its own (`main`) reads it directly.
    pd_globals: HashMap<String, (Value<'a>, Ty)>,
    /// The generated `kelp_init_pds` and `kelp_drop_pds`, present only for a
    /// program that declares a passdown. The init function is what `main`
    /// calls first; the drop function is what `atexit` registers, the only
    /// drop point an owned passdown has (`codegen_c.rs::write_pd_globals`).
    init_pds_fn: Option<LlvmFunction<'a>>,
    drop_pds_fn: Option<LlvmFunction<'a>>,
    /// The runtime's two saved `argv` globals (`runtime/kelp_rt.c`), which
    /// `main` writes and `process.args()` reads. They are `extern` here: the
    /// runtime object defines them, exactly as it does for the C backend's
    /// generated `main`.
    argc_global: Value<'a>,
    argv_global: Value<'a>,
    /// The source map, which `cap_plan` reads for the `.kp` call site a
    /// capability denial names. The C backend reads the same map for the same
    /// text (`codegen_c.rs::cap_plan`), so the two cannot name different
    /// places.
    sources: &'a SourceMap,
    /// The compiled-in allowlist array of each `(module, domain)` grant, keyed
    /// by that pair. The data is C in the per-program object
    /// (`program_c_source`); this is the symbol a check passes as `patterns`.
    cap_arrays: HashMap<(String, String), Value<'a>>,
    /// The capability check's C entry points (`kelp_cap_check_path` and
    /// friends), declared with the signature `runtime/cap_glue.c` defines.
    cap_fns: HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)>,
    /// The DWARF debug-information builder (MK7 S4 §4.4): the one place a
    /// `.kp` file and line become metadata. It owns the compile unit, the
    /// per-module `DIFile`s and the per-function `DISubprogram`s, and it is
    /// finalized before the module is verified or written.
    debug: DebugBuilder<'a>,
    /// The `DISubprogram` of each `.kp`-declared function, keyed by the same
    /// `Function::name` the lowering looks a function up by. A function with no
    /// entry here has no `.kp` source behind it (an `extern fun`, whose body is
    /// C and compiled separately, or a module whose text was never registered),
    /// so its instructions carry no location rather than a wrong one.
    debug_scopes: HashMap<String, Metadata<'a>>,
    /// The `DISubprogram` of the generated `kelp_init_pds`, when the program
    /// has passdowns with a registered source. The passdown initializers are
    /// emitted into that one function, and its scope is what makes their
    /// statements resolve to the `.kp` line that declared them.
    pd_init_scope: Option<Metadata<'a>>,
}

/// Every container instantiation a program mentions, the mirror of
/// `codegen_c::Instantiations` (MK7 S2D). Kept separate from the `datac` table
/// because a container is lowered as a *function set*, one per element or value
/// type, exactly as the C backend writes one C function set per instantiation.
#[derive(Default, Clone)]
struct Instantiations {
    lists: HashSet<Ty>,
    boxes: HashSet<Ty>,
    maps: HashSet<(Ty, Ty)>,
    arrays: HashSet<Ty>,
    /// Closure signatures (S2E). A closure's type is `{ fn, env, drop }`,
    /// monomorphized per signature exactly as the C backend monomorphizes its
    /// typedef, so one exists per signature the program mentions.
    closures: HashSet<Ty>,
    /// Closure signatures a plain function value is widened into somewhere, so
    /// each needs the thunk and the one-field environment that do the widening.
    coerced: HashSet<Ty>,
    /// Widened values whose function is *known* (a declared `fun` named as a
    /// value, or a literal that captured nothing, whose value is a `FnRef`
    /// too). Each gets a shim instead of the heap environment and the thunk.
    shims: HashSet<(String, Ty)>,
}

impl<'a> Gen<'a> {
    fn new(
        ctx: &'a Context,
        module: &'a Module<'a>,
        program: &'a Program,
        drops: &'a DropPlan,
        sources: &'a SourceMap,
        layout: &'a DataLayout,
        optimized: bool,
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
        // The widening's one-field environment (S2E): `{ ptr f }`, where `f` is
        // the code pointer a thunk calls through. One type for every signature,
        // since a pointer is a pointer here.
        let coerce_env_ty = ctx.struct_type("kelp_ClosureCoerceEnv");
        ctx.struct_set_body(coerce_env_ty, &[ptr]);
        let runtime = declare_runtime(ctx, module, bytes_ty, string_ty, view_ty);
        let panic = *runtime
            .get("kelp_panic")
            .expect("kelp_panic is declared with the runtime");
        let type_decls = type_decls(&program.structs, &program.enums);
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
            containers: HashMap::new(),
            map_entries: HashMap::new(),
            container_fns: HashMap::new(),
            clone_fns: HashMap::new(),
            cmp_fns: HashMap::new(),
            closure_fns: HashMap::new(),
            env_types: HashMap::new(),
            coerce_env_ty,
            libc: declare_libc(ctx, module),
            instantiations: collect_instantiations(program),
            clonable: crate::types::clonable_types(&type_decls),
            sortable: crate::types::sortable_types(&type_decls),
            pd_globals: HashMap::new(),
            init_pds_fn: None,
            drop_pds_fn: None,
            argc_global: module.add_external_global("kelp_argc", ctx.i32()),
            argv_global: module.add_external_global("kelp_argv", ctx.opaque_ptr()),
            sources,
            cap_arrays: HashMap::new(),
            cap_fns: declare_cap_fns(ctx, module),
            debug: DebugBuilder::new(module, optimized),
            debug_scopes: HashMap::new(),
            pd_init_scope: None,
        })
    }

    /// Declare every `datac` and `enum`, its drop, clone and comparator
    /// functions, the container function sets, the passdown globals, the
    /// capability allowlist declarations and every function.
    ///
    /// Declaring everything before any body is emitted means a construct this
    /// lowering cannot handle fails with that construct named, rather than with
    /// the first expression that mentions it.
    fn declare(&mut self) -> Result<(), String> {
        let program = self.program;
        // The container types first, so a `datac` or `enum` field that *is* one
        // resolves its own named struct rather than a refused type. A container
        // typedef is emitted before every `datac` in the C backend for the same
        // reason (a container holds its contents through a pointer).
        self.declare_container_types()?;
        // The aggregate types next, so a field this slice cannot lay out is
        // reported by its own construct rather than behind an unrelated std
        // declaration.
        self.declare_aggregates()?;
        // The environment struct of each capturing literal (S2E), after every
        // aggregate type exists and before any signature names the `ptr` that
        // carries it.
        self.declare_closure_envs()?;
        // A `pd` is a module-scope global, the generated `kelp_init_pds` that
        // fills it in declaration order, and (for an owned one) the `atexit`
        // registration that is its only drop point. Declared here, after the
        // aggregate and container types a `pd`'s value may name, and filled in
        // by `emit_pd_globals` once the drops it needs exist.
        self.declare_pd_globals()?;
        // Each `(module, domain)` grant's compiled-in allowlist array: the data
        // is C in the per-program object (`program_c_source`, the same text and
        // the same escaping the C backend writes), and this is the symbol a
        // check passes to `kelp_cap_check_*` as `patterns`.
        for grant in &program.caps {
            let symbol = cap_array_symbol(&grant.module, &grant.domain);
            let array = self.module.add_external_global(&symbol, self.ctx.opaque_ptr());
            self.cap_arrays
                .insert((grant.module.clone(), grant.domain.clone()), array);
        }
        // `program.caps` holds each module's granted patterns, which is data, not
        // a construct. The allowlist arrays are declared above and the check
        // itself is emitted at each gated call site (`cap_plan`,
        // `emit_cap_checks`), exactly where the C backend emits it.

        self.declare_drop_fns();
        // The derived deep-copy and order, then the container function sets: a
        // set's own `clone`/`sort`/drop calls the generated per-`datac`
        // `kelp_clone_<name>`/`kelp_cmp_<name>`, so both exist first.
        self.declare_clone_cmp_fns();
        self.declare_container_fns()?;
        // The closure machinery (S2E): the per-signature drop, the widening
        // thunk and its destructor, and the shim a known function is widened
        // through. Declared after the container sets because a container whose
        // element is a closure (`Box<closure(...)>`) calls the element's drop.
        self.declare_closure_fns()?;

        let ctx = self.ctx;

        for f in &program.functions {
            // An `extern fun` has no `.kp` body: its C text is supplied by the
            // compiler (`externs.rs`), so the function is *declared* here with
            // the body's own C signature and a call lowers as a call to it
            // (MK7 S3). The body then reaches the link through the object
            // `program_c_source` builds.
            if f.extern_ {
                self.declare_extern(f)?;
                continue;
            }
            let (params, ret) = if f.name == "main" {
                // The C backend's `main` is `int main(int, char**)`; this
                // mirrors the type the C standard cares about and returns 0.
                // The two parameters are not Kelp parameters: `main` saves
                // them into the runtime's `kelp_argc`/`kelp_argv`, which is
                // how `process.args()` reads this process's own argv (the
                // passdown mechanism cannot carry it, because the one function
                // that can take no hidden parameter is `main`).
                (vec![ctx.i32(), ctx.opaque_ptr()], ctx.i32())
            } else {
                let mut params: Vec<Type<'a>> = Vec::new();
                // A literal that captured something receives its environment as
                // a hidden first parameter, as a `void*` the body casts back to
                // the literal's own environment struct (A-4 phase E). The
                // environment's layout stays out of the signature, which is what
                // lets two closures of one signature share a code-pointer type.
                if f.env.is_some() {
                    params.push(ctx.opaque_ptr());
                }
                params.extend(
                    f.params
                        .iter()
                        .map(|(_, ty)| llvm_type(ctx, &self.structs, &self.containers, ty))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                let ret = llvm_ret_type(ctx, &self.structs, &self.containers, &f.ret)?;
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
        // The DWARF skeleton last, because it needs each function's LLVM value
        // to attach a subprogram to, and the passdown globals for the generated
        // `kelp_init_pds`.
        self.declare_debug();
        Ok(())
    }

    /// Build the DWARF skeleton (MK7 S4, §4.4): one compile unit for the
    /// module, one `DIFile` per `.kp` source, one `DISubprogram` per
    /// `.kp`-declared function.
    ///
    /// Nothing here is a source *location* yet. The locations are given to the
    /// builder as each statement is emitted (`FnGen::locate`), which is what
    /// puts the statement, not only the function, in the line table: the same
    /// resolution one `#line` per statement buys the C backend
    /// (`codegen_c.rs::write_line_marker`).
    ///
    /// The line and column come from the same `Span`s the C backend feeds to
    /// `diag::SourceMap::line_directive`, through the one accessor both share
    /// (`diag::SourceMap::path_line_col`), so the two backends cannot name
    /// different lines for one statement.
    ///
    /// A program whose sources were never registered gets no compile unit and
    /// no subprogram at all, and every instruction then carries no location.
    fn declare_debug(&mut self) {
        let program = self.program;
        let sources = self.sources;
        // The compile unit's own file: the module that declares `main`, since
        // that is the translation unit's entry point, falling back to the first
        // declaration. A program with no function to describe has no line to
        // name, so it gets no metadata rather than an empty compile unit.
        let main_module = program
            .functions
            .iter()
            .find(|f| f.name == "main" && !f.extern_)
            .or_else(|| program.functions.iter().find(|f| !f.extern_))
            .map(|f| f.module.clone());
        let Some(main_module) = main_module else {
            return;
        };
        // One `DIFile` per module the program declares a function in. A
        // `DILocation`'s file is its scope's, and a function belongs to exactly
        // one module, so one file per module is what every location inside its
        // functions resolves through.
        let mut files: HashMap<String, Metadata<'a>> = HashMap::new();
        for f in &program.functions {
            if files.contains_key(&f.module) {
                continue;
            }
            if let Some((path, _, _)) = sources.path_line_col(&f.module, f.span) {
                files.insert(f.module.clone(), self.debug.file(path));
            }
        }
        let Some(&main_file) = files.get(&main_module) else {
            return;
        };
        // The producer is the human-facing release name, padded the way every
        // other spelling of it is (`main.rs::release_name`).
        let producer = format!("kelpc {}", crate::release_name(crate::VERSION));
        let cu = self.debug.compile_unit(main_file, &producer);
        // A compile unit that is not named in `llvm.dbg.cu`, beside the `Debug
        // Info Version` flag, is dropped from the object: both halves are what
        // makes the sections exist.
        self.module.add_debug_compile_unit(cu);
        self.module.set_debug_info_version();

        for f in &program.functions {
            // An `extern fun` has no body in this module: its C text is
            // compiled into the program's own object (S3A), so there is no
            // instruction here to locate.
            if f.extern_ {
                continue;
            }
            let (Some(&file), Some((_, line, _))) =
                (files.get(&f.module), sources.path_line_col(&f.module, f.span))
            else {
                continue;
            };
            let ty = self.debug.subroutine_type(file);
            let sp = self.debug.subprogram(
                file,
                &f.name,
                &symbol_name(&f.name),
                file,
                line,
                ty,
            );
            if let Some(info) = self.funcs.get(&f.name) {
                self.debug.set_subprogram(info.func, sp);
            }
            self.debug_scopes.insert(f.name.clone(), sp);
        }

        // The passdown initializers are emitted into one generated function, so
        // they share one subprogram. Its file is the first passdown's, which is
        // exact for the common case of one `.kp` file and only approximate for a
        // program whose passdowns come from several: the *lines* of the
        // statements below are each passdown's own, and only the scope's file is
        // shared.
        if let Some(init_fn) = self.init_pds_fn {
            // Every initializer has to be locatable before the function gets a
            // subprogram: a function that carries debug info must have a
            // location on each of its calls (LLVM's verifier says so, because
            // the inliner needs one), and one initializer from a module with no
            // registered source would leave the whole function unverifiable.
            let all_located = program
                .pds
                .iter()
                .all(|pd| sources.path_line_col(&pd.module, pd.init.span).is_some());
            if let (true, Some(pd)) = (all_located, program.pds.first()) {
                if let (Some(&file), Some((_, line, _))) = (
                    files.get(&pd.module),
                    sources.path_line_col(&pd.module, pd.init.span),
                ) {
                    let ty = self.debug.subroutine_type(file);
                    let sp = self.debug.subprogram(
                        file,
                        "kelp_init_pds",
                        "kelp_init_pds",
                        file,
                        line,
                        ty,
                    );
                    self.debug.set_subprogram(init_fn, sp);
                    self.pd_init_scope = Some(sp);
                }
            }
        }
    }

    /// Point the builder at the `.kp` line `span` of `module` names, in
    /// `scope`, so every instruction emitted until the next call to this
    /// carries it (MK7 S4, §4.4).
    ///
    /// The LLVM half of `codegen_c.rs::write_line_marker`: `codegen_llvm.rs`
    /// calls it from the same places, with the same spans, so the two backends
    /// put a statement at the same source line.
    ///
    /// A span the source map cannot resolve, or a function with no subprogram,
    /// clears the location instead. An instruction that names no line is
    /// honest, and one that names a line it did not come from is not.
    fn locate(&self, scope: Option<Metadata<'a>>, module: &str, span: Span) {
        match (scope, self.sources.path_line_col(module, span)) {
            (Some(scope), Some((_, line, column))) => self
                .builder
                .set_debug_location(self.debug.location(line, column, scope)),
            _ => self.builder.clear_debug_location(),
        }
    }

    /// Finalize the DWARF metadata (`LLVMDIBuilderFinalize`).
    ///
    /// This must run after the last location is attached and before the module
    /// is verified or written, so `compile` calls it between `emit_all` and
    /// `Module::verify`.
    fn finalize_debug(&self) {
        self.debug.finalize();
    }

    /// Declare one module-scope global per `pd`, plus the two generated
    /// functions `codegen_c.rs::write_pd_globals` emits beside them (MK7 §3.1,
    /// §5).
    ///
    /// Each global has internal linkage and a zero initializer, which is C's
    /// `static <type> <name>;`: the storage is private to this object and the
    /// linker never sees the name. The name is the passdown's own
    /// `pd#<name>` identifier mangled once (`symbol_name`), so the storage a
    /// function with no hidden parameter reads is the same one the initializer
    /// filled.
    ///
    /// A `pd` initializer *is* code that runs, so a gated call inside one is
    /// checked against the *declaring* module's grant (`Pd::module` is the
    /// only carrier of that module, since an initializer is not a function).
    /// The walk that emits it carries that module for the same reason.
    fn declare_pd_globals(&mut self) -> Result<(), String> {
        let ctx = self.ctx;
        if self.program.pds.is_empty() {
            return Ok(());
        }
        let owned: Vec<bool> = self
            .program
            .pds
            .iter()
            .map(|pd| self.owns_memory(&pd.value_ty))
            .collect();
        for pd in &self.program.pds {
            let lty = llvm_type(ctx, &self.structs, &self.containers, &pd.value_ty)?;
            let global = self.module.add_global(&symbol_name(&pd.name), lty);
            global.set_initializer(ctx.const_null(lty));
            global.set_internal_linkage();
            self.pd_globals
                .insert(pd.name.clone(), (global, pd.value_ty.clone()));
        }
        let void_fn = ctx.fn_type(ctx.void(), &[], false);
        self.init_pds_fn = Some(self.module.add_function("kelp_init_pds", void_fn));
        // An owned passdown has no scope to die at, so its drop point is process
        // exit. A program whose passdowns all own nothing gets no handler at
        // all, exactly as the C backend emits none.
        if owned.iter().any(|owns| *owns) {
            self.drop_pds_fn = Some(self.module.add_function("kelp_drop_pds", void_fn));
        }
        Ok(())
    }

    /// Fill the two generated functions declared by [`Gen::declare_pd_globals`]:
    /// `kelp_drop_pds` frees every owned passdown in declaration order, and
    /// `kelp_init_pds` registers it with `atexit` and then runs each
    /// initializer in declaration order, storing the result into that
    /// passdown's global.
    ///
    /// The order is what makes a chained initializer (`pd val doubled =
    /// base * 2`) read the value an earlier one wrote. An initializer that
    /// diverges (`panic`) ends the init function, so the remaining passdowns
    /// are never assigned, which is the same fate the C backend's sequential
    /// statements have.
    fn emit_pd_globals(&self) -> Result<(), String> {
        let Some(init_fn) = self.init_pds_fn else {
            return Ok(());
        };
        if let Some(drop_fn) = self.drop_pds_fn {
            let entry = drop_fn.append_block("entry");
            self.builder.position_at_end(entry);
            for pd in &self.program.pds {
                if !self.owns_memory(&pd.value_ty) {
                    continue;
                }
                let (global, _) = *self
                    .pd_globals
                    .get(&pd.name)
                    .ok_or_else(|| no_value(&format!("the passdown '{}'", pd.qualified)))?;
                self.call_drop(global, &pd.value_ty)?;
            }
            self.builder.build_ret_void();
        }

        let entry = init_fn.append_block("entry");
        self.builder.position_at_end(entry);
        if let Some(drop_fn) = self.drop_pds_fn {
            // The registration is generated glue with no `.kp` statement of its
            // own, but it is a call inside a function that carries a subprogram,
            // and LLVM's verifier requires such a call to have a location. It is
            // attributed to the first passdown's initializer, which is the
            // passdown whose drop it registers, through the same helper the
            // initializer statements themselves use.
            let first = self.program.pds.first();
            match first {
                Some(pd) => self.locate(self.pd_init_scope, &pd.module, pd.init.span),
                None => self.builder.clear_debug_location(),
            }
            let (atexit, ty) = self.libc_fn("atexit")?;
            self.builder
                .build_call(atexit, ty, &[drop_fn.as_value()], "");
        }
        let mut diverged = false;
        for pd in &self.program.pds {
            let (global, _) = *self
                .pd_globals
                .get(&pd.name)
                .ok_or_else(|| no_value(&format!("the passdown '{}'", pd.qualified)))?;
            let fg = FnGen::for_pd_init(self, pd, init_fn, entry, global)?;
            diverged = fg.emit_pd_init()?;
            if diverged {
                break;
            }
        }
        if !diverged {
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// The gated-call plan for one call site, or `None` when the call is not a
    /// checked one. The mirror of `codegen_c.rs::Gen::cap_plan`, field for
    /// field on purpose: which calls are checked, against which domain, with
    /// which arguments and which pattern count, is the security-relevant
    /// decision, and two backends answering it differently would be a hole in
    /// one of them.
    ///
    /// The whole gate is frontend: resolution already refuses an ungranted
    /// name, so a call that reaches codegen is one the calling module may make.
    /// What the check adds is the *pattern* test at the call site, and a call
    /// that names no resource (`time.now_unix`, `rand.below`, `process.exit`)
    /// needs none, exactly as it gets none from the C backend.
    ///
    /// A gated stdlib module calling its own item is not a checked call site:
    /// the declaring module holds no grant, so it never appears in
    /// `program.caps` for that domain.
    fn cap_plan(
        &self,
        module: &str,
        callee: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<CapPlan> {
        let domain = self.program.gated.get(callee)?;
        let grant = self
            .program
            .caps
            .iter()
            .find(|g| g.module == module && g.domain == *domain)?;
        let arg_is_str: Vec<bool> = args
            .iter()
            .map(|a| crate::stdlib::is_str_like_ty(&a.ty))
            .collect();
        // The per-call table is keyed on the bare name the author wrote
        // (`read`, not `fs.read`), the qualified key being what `gated` holds.
        let bare = callee.rsplit('.').next().unwrap_or(callee);
        let checks = crate::stdlib::checked_args(domain, bare, &arg_is_str);
        if checks.is_empty() {
            return None;
        }
        // The call site, as the author wrote it: the same `locate` the C
        // backend calls, so a denial names the same `file:line:col`.
        let at = self
            .sources
            .locate(module, span)
            .unwrap_or_else(|| module.to_string());
        Some(CapPlan {
            domain: domain.clone(),
            pattern_count: grant.patterns.len(),
            at,
            checks,
        })
    }

    /// Declare one `extern fun` as the C function its compiler-supplied body
    /// defines (MK7 S3).
    ///
    /// The declared type is the **C ABI's** shape of that function, not its
    /// written spelling. An aggregate larger than two eightbytes comes back
    /// through a hidden pointer in the first argument slot, which is what clang
    /// compiles the body's `return` into; LLVM's own lowering of an aggregate
    /// *return* is not that, so declaring the return by value would produce a
    /// call that segfaults against the clang object (BUG-vN4tR, MK7 §3.3).
    ///
    /// Nothing here can see that object, so the agreement is kept by reading, and
    /// the shapes it depends on are stated rather than assumed:
    ///
    /// * every parameter is a pointer or a scalar. `infer.rs`'s
    ///   `implicit_borrow_wrap` turns a parameter that owns memory into a borrow,
    ///   and the runtime's own text uses `kelp_data->len` accordingly, so no
    ///   aggregate crosses *into* the C body by value. The guard below stays so
    ///   that a future extern whose parameter is a bare aggregate fails by name
    ///   rather than crossing wrongly.
    /// * the only aggregate returns in the whole extern table are `Bytes` and
    ///   `String`, both 24 bytes, so both take `sret`; `str` is a bare pointer
    ///   and every other return is a scalar.
    fn declare_extern(&mut self, f: &Function) -> Result<(), String> {
        let ctx = self.ctx;
        let ret_llvm = if f.ret == Ty::Unit {
            None
        } else {
            Some(llvm_type(ctx, &self.structs, &self.containers, &f.ret)?)
        };
        let sret = ret_llvm
            .map(|ty| crosses_c_boundary_by_value(&f.ret, ty, self.layout))
            .unwrap_or(false);
        let mut params: Vec<Type<'a>> = Vec::new();
        if sret {
            params.push(ctx.opaque_ptr());
        }
        for (pname, pty) in &f.params {
            let llvm_pty = llvm_type(ctx, &self.structs, &self.containers, pty)?;
            if crosses_c_boundary_by_value(pty, llvm_pty, self.layout) {
                return Err(refuse(
                    &format!("the extern '{}' taking '{}' by value", f.name, pname),
                    "S3",
                ));
            }
            params.push(llvm_pty);
        }
        // A struct that comes back through the hidden pointer returns `void`
        // here, and the destination is argument 0: that is the shape clang
        // compiles the C function's return into, and it is not what an LLVM
        // *value* return of the same struct lowers to (BUG-vN4tR).
        let ret = match (sret, ret_llvm) {
            (true, _) => ctx.void(),
            (false, Some(ty)) => ty,
            (false, None) => ctx.void(),
        };
        let ty = ctx.fn_type(ret, &params, false);
        let symbol = symbol_name(&f.name);
        let func = self.module.add_function(&symbol, ty);
        // The declarations live in the same table as the runtime helpers, and a
        // call reaches either through the same lookup (`emit_call`): a body and
        // a `kelp_rt.c` helper differ in where their text is compiled, not in how
        // the IR calls them.
        self.runtime.insert(
            symbol,
            RtFn {
                func,
                ty,
                yields: ret_llvm.is_some(),
                ret: ret_llvm,
                sret,
            },
        );
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
        // An array field is held *by value*, so its body has to be set before the
        // `datac` or payload that names it. `emit_array_body` recurses into the
        // arrays nested inside it, innermost first, exactly as `codegen_c.rs`
        // emits them just before the declaration that needs them.
        let mut emitted_arrays: HashSet<Ty> = HashSet::new();
        for (_, decl) in decls {
            let field_tys: Vec<Ty> = match decl {
                Decl::Record(s) => s.fields.iter().map(|f| f.ty.clone()).collect(),
                Decl::Sum(e) => e
                    .variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                    .collect(),
            };
            for ty in &field_tys {
                self.emit_array_body(ty, &mut emitted_arrays)?;
            }
            match decl {
                Decl::Record(s) => {
                    let ty = *self
                        .structs
                        .get(&s.name)
                        .expect("every datac got a type above");
                    let fields = s
                        .fields
                        .iter()
                        .map(|f| llvm_type(ctx, &self.structs, &self.containers, &f.ty))
                        .collect::<Result<Vec<_>, _>>()?;
                    ctx.struct_set_body(ty, &fields);
                }
                Decl::Sum(e) => self.declare_enum_body(e)?,
            }
        }
        // Every array the program uses that no by-value declaration pulled in
        // earlier: a local, a parameter, a return value, a `Box` element.
        let arrays: Vec<Ty> = self.instantiations.arrays.iter().cloned().collect();
        for ty in &arrays {
            self.emit_array_body(ty, &mut emitted_arrays)?;
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
                .map(|f| llvm_type(ctx, &self.structs, &self.containers, &f.ty))
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
            // A container's drop is one of the monomorphized functions this
            // slice generates into the module (`kelp_list_drop__i32`).
            Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..) => {
                let symbol = drop_fn_name(ty);
                match self.container_fns.get(&symbol).copied() {
                    Some((func, fty)) => (func, fty),
                    None => return Ok(()),
                }
            }
            // A closure owns the environment its captures were moved into, so
            // its drop is the per-signature generated one (S2E). A plain
            // function value is `Copy` and owns nothing, so it is the silence
            // below.
            Ty::Closure { .. } => {
                let symbol = drop_fn_name(ty);
                match self.closure_fns.get(&symbol).copied() {
                    Some((func, fty)) => (func, fty),
                    None => return Ok(()),
                }
            }
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
    /// Canonical id of the module the function was declared in. The capability
    /// gate is per module, so a gated call site has to know whose grant it is
    /// checked against (MK7 S3, `Gen::gated_check_needed`).
    module: String,
    ret_ty: Ty,
    /// The function's `tir` body, borrowed for `'a` from the program so the
    /// walk does not hold a borrow of `self`. `None` in the synthetic walk a
    /// `pd` initializer is emitted through, whose single expression is in
    /// `pd_init`.
    body: Option<&'a Block>,
    /// The `pd` initializer this walk emits, when it is the generated
    /// `kelp_init_pds` rather than a function body, and the global it assigns
    /// (`tir::Pd::init`, MK7 §5).
    pd_init: Option<&'a Expr>,
    pd_global: Option<Value<'a>>,
    /// The parameter list, in declaration order (a `fun` literal's environment
    /// first, when it has one).
    params: Vec<(String, Ty)>,
    /// The LLVM environment struct of this literal, when it captured something
    /// (S2E): `Some` makes the generated function take a hidden first `ptr`
    /// parameter and the body's `Capture` reads project into that struct.
    env_ty: Option<Type<'a>>,
    /// The slot holding the hidden environment pointer, bound in the entry
    /// block. One location, so `mem2reg` can promote it out.
    env_slot: Option<Value<'a>>,
    /// The `DISubprogram` this function's source locations hang from, when it
    /// has one (MK7 S4). `None` leaves every instruction of the body without a
    /// location.
    debug_scope: Option<Metadata<'a>>,
    /// The `.kp` span that names this function's own line, which is the
    /// location its entry block starts under. `None` for the synthetic walk a
    /// passdown initializer is emitted through, which has a span of its own
    /// instead.
    decl_span: Option<Span>,
}

impl<'g, 'a> FnGen<'g, 'a> {
    fn new(g: &'g Gen<'a>, f: &'a Function) -> Result<Self, String> {
        let info = g
            .funcs
            .get(&f.name)
            .cloned()
            .ok_or_else(|| no_value("a declared function"))?;
        let entry = info.func.append_block("entry");
        let env_ty = match &f.env {
            Some(_) => Some(
                *g.env_types
                    .get(&f.name)
                    .ok_or_else(|| no_value(&format!("the environment of '{}'", f.name)))?,
            ),
            None => None,
        };
        Ok(FnGen {
            g,
            cur_fn: info.func,
            entry,
            cur: entry,
            terminated: false,
            scopes: Vec::new(),
            fn_name: f.name.clone(),
            module: f.module.clone(),
            ret_ty: f.ret.clone(),
            body: Some(&f.body),
            pd_init: None,
            pd_global: None,
            params: f.params.clone(),
            env_ty,
            env_slot: None,
            debug_scope: g.debug_scopes.get(&f.name).copied(),
            decl_span: Some(f.span),
        })
    }

    /// The synthetic walk a `pd` initializer is emitted through (MK7 §5): no
    /// body, no parameters, and one expression whose result is stored into the
    /// passdown's own global.
    ///
    /// `fn_name` is empty and the module is the declaring one, exactly as
    /// `codegen_c.rs::write_pd_globals` builds its `Gen`: the drop plan has no
    /// entry keyed to a `pd` (`borrowck.rs` keys by function, and an initializer
    /// is not one), and a gated call inside the initializer is checked against
    /// the module that declared it, which is the only carrier of that module.
    fn for_pd_init(
        g: &'g Gen<'a>,
        pd: &'a Pd,
        cur_fn: LlvmFunction<'a>,
        entry: BasicBlock<'a>,
        global: Value<'a>,
    ) -> Result<Self, String> {
        Ok(FnGen {
            g,
            cur_fn,
            entry,
            cur: entry,
            terminated: false,
            scopes: Vec::new(),
            fn_name: String::new(),
            module: pd.module.clone(),
            ret_ty: pd.value_ty.clone(),
            body: None,
            pd_init: Some(&pd.init),
            pd_global: Some(global),
            params: Vec::new(),
            env_ty: None,
            env_slot: None,
            debug_scope: g.pd_init_scope,
            decl_span: Some(pd.init.span),
        })
    }

    /// Aim the builder at the `.kp` line `span` names, so every instruction
    /// emitted until the next call to this carries it (MK7 S4, §4.4).
    ///
    /// This is the LLVM half of `codegen_c.rs::write_line_marker`, called from
    /// the same places with the same span, so the two backends put a statement
    /// at the same source line. A span the source map cannot resolve, or a
    /// function with no subprogram, clears the location instead: an instruction
    /// that names no line is honest, and one that names a line it did not come
    /// from is not.
    fn locate(&self, span: Span) {
        self.g.locate(self.debug_scope, &self.module, span);
    }

    /// Start the function under its own declaration line, which is what makes
    /// the entry block, and so the prologue, resolve to the function rather
    /// than to the previous function's last statement.
    fn locate_entry(&self) {
        match self.decl_span {
            Some(span) => self.locate(span),
            None => self.g.builder.clear_debug_location(),
        }
    }

    /// Emit one `pd` initializer and store its value into the passdown's
    /// global. Returns whether it diverged, which is what stops the enclosing
    /// `kelp_init_pds` from assigning a later passdown into a block that has
    /// already ended.
    fn emit_pd_init(mut self) -> Result<bool, String> {
        self.g.builder.position_at_end(self.entry);
        // The initializer is a `.kp` expression, so it resolves to its own line
        // even though it is emitted into a generated function (MK7 S4).
        self.locate_entry();
        self.scopes.push(HashMap::new());
        let init = self
            .pd_init
            .ok_or_else(|| no_value("a passdown initializer"))?;
        let value = self.emit_expr(init)?;
        if !self.terminated {
            let value = value.ok_or_else(|| no_value("the value of a passdown initializer"))?;
            let global = self
                .pd_global
                .ok_or_else(|| no_value("the storage of a passdown initializer"))?;
            self.g.builder.build_store(value, global);
        }
        Ok(self.terminated)
    }

    fn emit(mut self) -> Result<(), String> {
        self.g.builder.position_at_end(self.entry);
        // The entry block starts under the function's own line: the parameter
        // slots and the `kelp_init_pds` call are part of the function, so they
        // must not carry whatever the previously emitted function left behind
        // (MK7 S4).
        self.locate_entry();
        self.scopes.push(HashMap::new());

        // A `pd` init function's body is the initializer walk, called first by
        // `main`; the passdowns themselves are globals, so nothing is
        // registered here.
        if self.pd_init.is_some() {
            self.emit_pd_init()?;
            return Ok(());
        }

        // `main` runs the passdown initializers before its first statement,
        // exactly where `codegen_c.rs::write_function` inserts
        // `kelp_init_pds();`, and saves its own `argc`/`argv` into the
        // runtime's globals, where `process.args()` reads them. `main`
        // receives no hidden parameter, so it reads the generated globals
        // directly; the slots a referencing function gets are bound below for
        // every other function.
        if self.fn_name == "main" {
            if let Some(init) = self.g.init_pds_fn {
                let ty = self.g.ctx.fn_type(self.g.ctx.void(), &[], false);
                self.g.builder.build_call(init, ty, &[], "");
            }
            let argc = self.cur_fn.get_param(0);
            let argv = self.cur_fn.get_param(1);
            self.g.builder.build_store(argc, self.g.argc_global);
            self.g.builder.build_store(argv, self.g.argv_global);
        }

        // A literal that captured something receives its environment as a
        // hidden first parameter (S2E). Bind it into a slot of its own, so
        // every `Capture` read loads one location.
        let param_offset = if self.env_ty.is_some() {
            let raw = self.cur_fn.get_param(0);
            let slot = self.temp_alloca(self.g.pty(), "env");
            self.g.builder.build_store(raw, slot);
            self.env_slot = Some(slot);
            1
        } else {
            0
        };

        // Bind each parameter into its own slot, so its name is a load like any
        // other local.
        if self.fn_name != "main" {
            for (i, (name, ty)) in self.params.clone().iter().enumerate() {
                let value = self.cur_fn.get_param(i + param_offset);
                let slot = self.alloca(ty, name)?;
                self.g.builder.build_store(value, slot);
                self.scopes
                    .last_mut()
                    .expect("the function scope is open")
                    .insert(name.clone(), slot);
            }
        }

        let body = self
            .body
            .ok_or_else(|| no_value("the body of a function"))?;
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
            // The tail is an expression in statement position here, and the C
            // backend marks it as one (`codegen_c.rs::write_block_body`).
            self.locate(tail.span);
            // A body whose tail is a `panic` diverges: `infer.rs` accepts it
            // against any declared return type, so it is a statement rather
            // than a returned value (`codegen_c.rs::write_function_body`
            // draws the same line). The call ends the block itself, so no
            // `ret` and no drop follows it.
            if is_diverging_tail(tail) {
                self.emit_expr(tail)?;
                return Ok(());
            }
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
        if let Some(tail) = &block.tail {
            self.locate(tail.span);
        }
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
        // One location per statement, from the span the statement itself
        // carries: a `Let` is the initializer's span (its first token *is* the
        // `val`), a `Return` carries its own, a `While` its condition's. The
        // same four spans `codegen_c.rs::write_stmt` writes its `#line` from.
        match stmt {
            Stmt::Let { init, .. } => self.locate(init.span),
            Stmt::Return { span, .. } => self.locate(*span),
            Stmt::While { cond, .. } => self.locate(cond.span),
            Stmt::Expr(e) => self.locate(e.span),
        }
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
            ExprKind::Call { callee, args } => self.emit_call(callee, args, &e.ty, e.span),
            // `[a, b, c]`: the array struct is built field by field in order,
            // the same value the C compound literal is.
            ExprKind::ArrayLit(elems) => Ok(Some(self.emit_array_lit(elems, &e.ty)?)),
            // `[v; N]`: one call to the generated fill helper, so a large
            // repeat is a loop rather than N initializers.
            ExprKind::ArrayRepeat { value } => {
                Ok(Some(self.emit_array_repeat(value, &e.ty)?))
            }
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
                Ok(Some(self.emit_field_read(e, base, field, &e.ty)?))
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
            // an owned payload be bound as a borrow rather than moved out, and
            // what lets the move out by value clear the slot it came from.
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => Ok(Some(self.emit_enum_field(e, base, *variant, *field, &e.ty)?)),
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
            // A borrow (`infer.rs::check_call_arg` synthesizes one for a
            // parameter whose type resolves to a shared or exclusive borrow,
            // and `&x`/`&mut x` write one directly). `&str` is the pointer
            // itself, as the C backend writes it, because there is no second
            // indirection; every other pointee is the address of the place it
            // borrows, which is the one indirection `getelementptr` needs and
            // the pointer the runtime's `const kelp_Bytes*` parameters take.
            // `Ty::Ref` is one pointer whatever the pointee (`llvm_type`), so
            // the place's address is the borrow, scalar or `datac` alike.
            ExprKind::Ref { expr: sub, .. } => {
                if is_str_ref(&e.ty) {
                    return Ok(Some(self.value_of(sub)?));
                }
                Ok(Some(self.place_of(sub)?))
            }
            // `*p`: the pointee is whatever the pointer addresses, a `datac`, a
            // scalar, `str` or a container, so the load is the pointee type's
            // own (`Ty::Str` loads the `char*` a `str` value is).
            ExprKind::Deref { expr: sub } => {
                // `*p` on a `&str` is the view itself: a `str` is already the
                // pointer, so there is no second indirection to load
                // (`codegen_c.rs::expr_value` returns `expr_value(sub)` for the
                // same node). This is the shape a `pd` of type `str` reads
                // through, and a `&str` parameter read too.
                if e.ty == Ty::Str {
                    return Ok(Some(self.value_of(sub)?));
                }
                let ptr = self.value_of(sub)?;
                let lty = self.lty(&e.ty)?;
                Ok(Some(self.g.builder.build_load(lty, ptr, "deref")))
            }
            ExprKind::Template { newline, pieces } => {
                self.emit_template(*newline, pieces)?;
                Ok(None)
            }
            // A declared `fun` (or a literal that captured nothing) used as a
            // value (S2E): the C function is the value, spelled as its address.
            ExprKind::FnRef(key) => Ok(Some(self.emit_fn_ref(key)?)),
            // A capturing `fun` literal: the heap environment, one field per
            // capture in capture order, then the fat pointer.
            ExprKind::ClosureLit { key, captures } => {
                Ok(Some(self.emit_closure_lit(key, captures, &e.ty)?))
            }
            // One field of the enclosing closure's environment. It is a
            // *place* (the environment owns the value), so the read is the
            // field's own type loaded out of that address.
            ExprKind::Capture { index } => {
                let place = self.capture_place(*index)?;
                let lty = self.lty(&e.ty)?;
                Ok(Some(self.g.builder.build_load(lty, place, "capture")))
            }
            // A plain function value where a closure is expected: the widening
            // (S2E), a shim for a known function and a thunk for one whose
            // identity the call knows.
            ExprKind::FnToClosure { func } => {
                Ok(Some(self.emit_fn_to_closure(func, &e.ty)?))
            }
            ExprKind::IndirectCall { callee, args } => {
                self.emit_indirect_call(callee, args, &e.ty)
            }
        }
    }

    /// Read one `datac` field: the base's address, a `getelementptr` to the
    /// field's index, then a load of the field's own type.
    ///
    /// `ty` is the field expression's `tir` type, which is the pointee type the
    /// load needs and does not have to be recovered from the struct table.
    fn emit_field_read(
        &mut self,
        e: &Expr,
        base: &Expr,
        field: &str,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        // A chain rooted at an rvalue that owns memory (`mkName().r`) has
        // nowhere to hang the drop of the temporary: nothing else owns it and
        // nothing else frees it. The root is copied into a slot, the field is
        // read out of that copy, the temporary is dropped, and the value is
        // handed on, so the temporary and every buffer its other fields own die
        // at the end of the expression that read through it. This is
        // `codegen_c.rs::staged_field_value`, and it is the reason a two-owned-
        // field `datac` is the interesting case: the drop is at the temporary,
        // not at the field.
        //
        // The value is read out before the drop runs, because a drop frees
        // memory. `infer.rs` already refuses to read an *owned* field by value,
        // so the value is never the buffer the drop frees.
        if let Some(root) = temporary_root(base) {
            if self.g.owns_memory(&root.ty) {
                let root_ty = root.ty.clone();
                let lty = self.lty(&root_ty)?;
                let slot = self.temp_alloca(lty, "field.root");
                let value = self.value_of(root)?;
                self.g.builder.build_store(value, slot);
                let base_ptr = self.chain_addr(slot, base)?;
                let (struct_ty, index) = self.field_slot(&base.ty, field)?;
                let field_ptr =
                    self.g
                        .builder
                        .build_struct_gep(struct_ty, base_ptr, index, field);
                let lty = self.lty(ty)?;
                let read = self.g.builder.build_load(lty, field_ptr, field);
                let out = self.temp_alloca(lty, "field.value");
                self.g.builder.build_store(read, out);
                self.g.call_drop(slot, &root_ty)?;
                return Ok(self.g.builder.build_load(lty, out, "field"));
            }
        }
        let (ptr, struct_ty, index) = self.field_place(base, field)?;
        let field_ptr = self
            .g
            .builder
            .build_struct_gep(struct_ty, ptr, index, field);
        let lty = self.lty(ty)?;
        let read = self.g.builder.build_load(lty, field_ptr, field);
        // A partial move copies the field out and zeroes the slot it came
        // from. The binding keeps its value as a whole and is freed at scope
        // exit as it always was, so without this the free would reach the
        // buffer the moved value now owns, the same clear a whole-binding
        // move does in `load_ident`, one level down.
        match self.g.drops.clear_at(&self.fn_name, e) {
            // Not a partial move: the read copies nothing out, so the slot
            // keeps its value and the binding's own drop frees it.
            None => {}
            // The field this read copied out: zero the slot, so the binding's
            // drop does not free the value a second time.
            Some(Projection::Field(moved)) if moved == field => {
                self.g.builder.build_store(self.g.ctx.const_null(lty), field_ptr);
            }
            // A projection the backend has no clear for. Zeroing a slot that
            // still holds its value, or skipping the zero of one that does
            // not, frees the same buffer twice, so an arm per variant is what
            // has to be filled in rather than passed over.
            Some(Projection::Field(moved)) => panic!(
                "codegen_llvm: a move of field '{moved}' was recorded at a read of '{field}'"
            ),
            Some(Projection::EnumPayload { name, .. }) => panic!(
                "codegen_llvm: a move of payload '{name}' was recorded at a read of field '{field}'"
            ),
        }
        Ok(read)
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

    /// A declared `fun` (or a literal that captured nothing) named as a value
    /// (S2E): the function's own address, which *is* the value. Nothing is
    /// allocated and nothing is owned.
    fn emit_fn_ref(&self, key: &str) -> Result<Value<'a>, String> {
        self.g
            .funcs
            .get(key)
            .map(|info| info.func.as_value())
            // Only an `extern fun` can be missing here: an `extern fun` is a
            // declaration the runtime owns, which is the S3 slice, and its name
            // is the honest refusal.
            .ok_or_else(|| refuse(&format!("the function value '{key}'"), "S3"))
    }

    /// A capturing `fun` literal (S2E): the environment is heap (MK7 §3.3
    /// spells the pointer out), one field per capture in capture order, and the
    /// value is the fat pointer `{ fn, env, drop }`.
    fn emit_closure_lit(
        &mut self,
        key: &str,
        captures: &[Expr],
        sig: &Ty,
    ) -> Result<Value<'a>, String> {
        let func = self
            .g
            .funcs
            .get(key)
            .cloned()
            .ok_or_else(|| no_value(&format!("the literal '{key}'")))?;
        let closure_ty = self.g.closure_value_ty(sig)?;
        let env_ty = *self
            .g
            .env_types
            .get(key)
            .ok_or_else(|| no_value(&format!("the environment of '{key}'")))?;
        let env = self.g.malloc_for(env_ty)?;
        self.cur = self.g.check_alloc(env, self.cur_fn)?;
        for (index, init) in captures.iter().enumerate() {
            let value = self.value_of(init)?;
            let field = self.g.builder.build_struct_gep(
                env_ty,
                env,
                index as u32,
                &capture_field(index),
            );
            self.g.builder.build_store(value, field);
        }
        let drop = self.g.closure_symbol(&crate::types::env_drop_name(key))?;
        let mut agg = self.g.ctx.undef(closure_ty);
        agg = self.g.builder.build_insert_value(agg, func.func.as_value(), 0, "fn");
        agg = self.g.builder.build_insert_value(agg, env, 1, "env");
        agg = self.g.builder.build_insert_value(agg, drop.as_value(), 2, "drop");
        Ok(agg)
    }

    /// The widening of a plain function value into a closure (S2E). A *known*
    /// function takes the shim, which ignores the environment, so the value
    /// carries `null` there and allocates nothing; a value whose identity only
    /// the call knows gets the one-field heap environment and the thunk that
    /// reads the code pointer back out of it.
    fn emit_fn_to_closure(&mut self, func: &Expr, sig: &Ty) -> Result<Value<'a>, String> {
        let closure_ty = self.g.closure_value_ty(sig)?;
        let null = self.g.ctx.const_null(self.g.pty());
        if let ExprKind::FnRef(key) = &func.kind {
            let shim = self.g.closure_symbol(&shim_symbol(key, sig))?;
            let mut agg = self.g.ctx.undef(closure_ty);
            agg = self.g.builder.build_insert_value(agg, shim.as_value(), 0, "fn");
            agg = self.g.builder.build_insert_value(agg, null, 1, "env");
            agg = self.g.builder.build_insert_value(agg, null, 2, "drop");
            return Ok(agg);
        }
        let code = self.value_of(func)?;
        let env_ty = self.g.coerce_env_ty;
        let env = self.g.malloc_for(env_ty)?;
        self.cur = self.g.check_alloc(env, self.cur_fn)?;
        let slot = self
            .g
            .builder
            .build_struct_gep(env_ty, env, 0, "coerce.f");
        self.g.builder.build_store(code, slot);
        let thunk = self.g.closure_symbol(&coerce_thunk_symbol(sig))?;
        let drop = self.g.closure_symbol(&coerce_drop_symbol(sig))?;
        let mut agg = self.g.ctx.undef(closure_ty);
        agg = self.g.builder.build_insert_value(agg, thunk.as_value(), 0, "fn");
        agg = self.g.builder.build_insert_value(agg, env, 1, "env");
        agg = self.g.builder.build_insert_value(agg, drop.as_value(), 2, "drop");
        Ok(agg)
    }

    /// A call through a function value (S2E): a plain function pointer is the
    /// callee, and a closure is the fat pointer, so the call names its code
    /// pointer and passes the environment as the hidden first argument
    /// (`(c.fn)(c.env, ...)`, the shape `codegen_c.rs` writes).
    fn emit_indirect_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        ret: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        // A closure value or a borrow of one: the code pointer and the
        // environment come out of the same fat pointer the C backend projects.
        if let Some(sig) = closure_callee_sig(&callee.ty) {
            // The callee is read before the arguments, as the C backend binds
            // it to a temporary in front of the call, so a field or a
            // dereference is evaluated once.
            let (code, env) = self.closure_code_and_env(callee)?;
            let (params, ret_lty) = self.g.closure_sig_types(&sig)?;
            let mut all = Vec::with_capacity(params.len() + 1);
            all.push(env);
            let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
            for a in args {
                all.push(self.arg_value(a, &mut held)?);
            }
            let mut call_params = Vec::with_capacity(params.len() + 1);
            call_params.push(self.g.pty());
            call_params.extend(params);
            let call_ty = self.g.ctx.fn_type(ret_lty, &call_params, false);
            let yields = *ret != Ty::Unit;
            let name = if yields { "call" } else { "" };
            let v = self
                .g
                .builder
                .build_indirect_call(code, call_ty, &all, name);
            for (slot, ty) in held {
                self.g.call_drop(slot, &ty)?;
            }
            return Ok(if yields { Some(v) } else { None });
        }

        // A plain function value: the pointer itself is the callee. A borrow of
        // one holds the address of the function pointer, so the call loads it
        // first (`*p`), exactly as reading the pointee would.
        let code = match &callee.ty {
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Fn { .. }) => {
                let p = self.value_of(callee)?;
                self.g.builder.build_load(self.g.pty(), p, "callee")
            }
            _ => self.value_of(callee)?,
        };
        let (params, ret_lty) = self.g.fn_value_sig(&callee.ty)?;
        let mut all: Vec<Value<'a>> = Vec::with_capacity(args.len());
        let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
        for a in args {
            all.push(self.arg_value(a, &mut held)?);
        }
        let call_ty = self.g.ctx.fn_type(ret_lty, &params, false);
        let yields = *ret != Ty::Unit;
        let name = if yields { "call" } else { "" };
        let v = self
            .g
            .builder
            .build_indirect_call(code, call_ty, &all, name);
        for (slot, ty) in held {
            self.g.call_drop(slot, &ty)?;
        }
        Ok(if yields { Some(v) } else { None })
    }

    /// The code pointer and the environment of a closure callee (S2E). A
    /// `Ty::Closure` callee is read as a *place* (borrowck guarantees it), so
    /// the fat pointer is projected rather than copied out: the binding that
    /// owns the environment keeps it. A `Ty::Ref` callee is already the
    /// pointer to one.
    fn closure_code_and_env(&mut self, callee: &Expr) -> Result<(Value<'a>, Value<'a>), String> {
        let ptr = self.g.pty();
        let (base, ty) = if matches!(callee.ty, Ty::Ref { .. }) {
            (self.value_of(callee)?, self.g.closure_value_ty(&callee.ty)?)
        } else {
            let ty = self.g.closure_value_ty(&callee.ty)?;
            let place = if is_place_expr(callee) {
                self.place_of(callee)?
            } else {
                let value = self.value_of(callee)?;
                let slot = self.temp_alloca(ty, "callee");
                self.g.builder.build_store(value, slot);
                slot
            };
            (place, ty)
        };
        let code = self.g.builder.build_load(
            ptr,
            self.g.builder.build_struct_gep(ty, base, 0, "fn"),
            "fn",
        );
        let env = self.g.builder.build_load(
            ptr,
            self.g.builder.build_struct_gep(ty, base, 1, "env"),
            "env",
        );
        Ok((code, env))
    }

    /// The address of one field of the enclosing closure's environment (S2E).
    /// The environment owns the capture, so this is a *place*: an owned capture
    /// is borrowed rather than moved out.
    fn capture_place(&mut self, index: usize) -> Result<Value<'a>, String> {
        let slot = self
            .env_slot
            .ok_or_else(|| no_value("a capture outside a closure"))?;
        let env_ty = self
            .env_ty
            .ok_or_else(|| no_value("a capture outside a closure"))?;
        let raw = self.g.builder.build_load(self.g.pty(), slot, "env");
        Ok(self.g.builder.build_struct_gep(
            env_ty,
            raw,
            index as u32,
            &capture_field(index),
        ))
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
    ///
    /// An owned payload read by value is a partial move (A-11), exactly as an
    /// owned `datac` field read is: the value is copied out and the slot it came
    /// from is zeroed, so the enum's own drop frees the payloads the move left
    /// behind and nothing where the value now lives. `e` is the node being
    /// rendered, which is the identity the plan's clear is keyed on.
    fn emit_enum_field(
        &mut self,
        e: &Expr,
        base: &Expr,
        variant: usize,
        field: usize,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let ptr = self.enum_field_place(base, variant, field)?;
        let lty = self.lty(ty)?;
        let read = self.g.builder.build_load(lty, ptr, "payload");
        match self.g.drops.clear_at(&self.fn_name, e) {
            // Not a partial move: the read copies nothing out, so the slot
            // keeps its value and the binding's own drop frees it.
            None => {}
            // The payload this read copied out: zero the slot.
            Some(Projection::EnumPayload {
                variant: moved_variant,
                field: moved_field,
                ..
            }) if *moved_variant == variant && *moved_field == field => {
                self.g
                    .builder
                    .build_store(self.g.ctx.const_null(lty), ptr);
            }
            // A projection the backend has no clear for. Zeroing a slot that
            // still holds its value, or skipping the zero of one that does not,
            // frees the same buffer twice, so an arm per variant is what has to
            // be filled in rather than passed over.
            Some(Projection::Field(moved)) => panic!(
                "codegen_llvm: a move of field '{moved}' was recorded at a read of payload {variant}.{field}"
            ),
            Some(Projection::EnumPayload {
                variant: moved_variant,
                field: moved_field,
                ..
            }) => panic!(
                "codegen_llvm: a move of payload {moved_variant}.{moved_field} was recorded at a read of payload {variant}.{field}"
            ),
        }
        Ok(read)
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
        let (struct_ty, index) = self.field_slot(&base.ty, field)?;
        let ptr = self.place_of(base)?;
        Ok((ptr, struct_ty, index))
    }

    /// The LLVM struct type and field index a `datac` field resolves to, from
    /// the base's type. Separated from [`Self::field_place`] because the staged
    /// temporary path has the base's *address* already and only needs the
    /// projection.
    fn field_slot(&self, base_ty: &Ty, field: &str) -> Result<(Type<'a>, u32), String> {
        let struct_name = struct_name_of_ty(base_ty)
            .ok_or_else(|| refuse_type(base_ty))?
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
        Ok((struct_ty, index as u32))
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
                let slot = match self.lookup(name) {
                    Some(slot) => slot,
                    None => self
                        .pd_place(name)
                        .ok_or_else(|| no_value(&format!("the local '{name}'")))?,
                };
                if is_borrow_ty(&e.ty) {
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
            // A field of the enclosing closure's environment (S2E): the
            // environment owns the capture, so the field is its address.
            ExprKind::Capture { index } => self.capture_place(*index),
            // A borrow in a place position is already an address.
            ExprKind::Ref { expr: sub, .. } => self.place_of(sub),
            ExprKind::Deref { expr: sub } => self.value_of(sub),
            _ => {
                // A borrow-typed rvalue is already an address: a call returning
                // `&T` (`List#get`/`Map#get` on an owned element) yields the
                // pointer itself, which is exactly what a field read or a chain
                // step wants. Staging it into a slot would hand back the
                // address of the *pointer* instead of the pointee.
                if matches!(e.ty, Ty::Ref { .. }) {
                    return self.value_of(e);
                }
                // A staged temporary that owns memory has to be freed, and the
                // sites that own that duty are the borrow staging
                // (`borrow_place`) and the value-position field read
                // (`emit_field_read`); both intercept a chain rooted at an owned
                // rvalue before it reaches here. Reaching one anyway means a
                // shape no site claimed, so it is refused by name rather than
                // silently leaked: an owned temporary nobody frees is a leak,
                // and a hidden one is worse than a build failure.
                if self.g.owns_memory(&e.ty) {
                    return Err(refuse("'an owned temporary' at a place this lowering does not stage", "S3"));
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
        llvm_type(self.g.ctx, &self.g.structs, &self.g.containers, ty)
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
        if matches!(a.kind, ExprKind::Ref { .. }) && !is_str_ref(&a.ty) {
            return self.borrow_place(a, held);
        }
        self.value_of(a)
    }

    /// The address a borrow expression denotes, staging an owned temporary when
    /// the borrow hangs off one.
    ///
    /// `mkName().name` borrows a field of a `datac` nothing else owns: the
    /// temporary is copied into a slot of its own, the field's address is taken
    /// inside that copy, and the slot is handed back in `held` for the caller to
    /// free after its operation (the call or the `printf`). This is
    /// `codegen_c.rs::stage_argument`'s borrow arm and
    /// `stage_borrowed_field`, which stage the *whole* temporary and not the
    /// field, so a `datac` that owns two buffers frees both.
    ///
    /// A borrow that is not rooted at an owned rvalue is an ordinary place, so
    /// the question is only whether the chain's root is one.
    fn borrow_place(
        &mut self,
        arg: &Expr,
        held: &mut Vec<(Value<'a>, Ty)>,
    ) -> Result<Value<'a>, String> {
        if let ExprKind::Ref { expr: sub, .. } = &arg.kind {
            if let Some(root) = temporary_root(sub) {
                if self.g.owns_memory(&root.ty) {
                    let root_ty = root.ty.clone();
                    let lty = self.lty(&root_ty)?;
                    let slot = self.temp_alloca(lty, "borrow.tmp");
                    let value = self.value_of(root)?;
                    self.g.builder.build_store(value, slot);
                    let ptr = self.chain_addr(slot, sub)?;
                    held.push((slot, root_ty));
                    return Ok(ptr);
                }
            }
        }
        self.place_of(arg)
    }

    /// One argument of a runtime or extern call. An owned aggregate over 16
    /// bytes is spelled as a pointer, which is the only way such an argument
    /// crosses to C (BUG-vN4tR, MK7 §3.3): the value is staged into a slot of its
    /// own and the slot's address is what crosses. The value is *moved* into the
    /// call, so unlike `arg_value`'s borrow staging the slot is not freed
    /// afterwards. Everything else follows `arg_value`.
    fn rt_arg_value(
        &mut self,
        a: &Expr,
        held: &mut Vec<(Value<'a>, Ty)>,
    ) -> Result<Value<'a>, String> {
        if is_aggregate_ty(&a.ty) {
            let lty = self.lty(&a.ty)?;
            if crosses_c_boundary_by_value(&a.ty, lty, self.g.layout) {
                let slot = self.temp_alloca(lty, "arg.byref");
                let value = self.value_of(a)?;
                self.g.builder.build_store(value, slot);
                return Ok(slot);
            }
        }
        self.arg_value(a, held)
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
        // silently. S1 refuses it by name rather than miscompiling it, and the
        // frontend refuses a raw NUL in a string literal before this point.
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
        let lty = self.lty(&e.ty)?;
        let slot = match self.lookup(name) {
            Some(slot) => slot,
            // A `pd` global named by a function with no hidden parameter of its
            // own (`main`, or another `pd`'s initializer). Its storage outlives
            // every frame, so it is never a move source and is never cleared.
            None => match self.pd_place(name) {
                Some(global) => return Ok(self.g.builder.build_load(lty, global, name)),
                None => return Err(no_value(&format!("the local '{name}'"))),
            },
        };
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

    /// The module-scope global a `pd` names, when the name is not a local: the
    /// hidden parameter a referencing function receives is bound in a scope
    /// like any other parameter, so a miss here is `main` or a `pd` initializer
    /// (`infer.rs::PdInfo::arg` hands those the global's own address).
    fn pd_place(&self, name: &str) -> Option<Value<'a>> {
        self.g.pd_globals.get(name).map(|(global, _)| *global)
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
                let slot = match self.lookup(name) {
                    Some(slot) => slot,
                    None => self
                        .pd_place(name)
                        .ok_or_else(|| no_value(&format!("the local '{name}'")))?,
                };
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
            // A store through a borrow: `*p = v` (or `(*p).field = v` through
            // the `FieldAccess` arm above, whose `place_of` steps through the
            // pointer). The pointer is the borrow's own value, so the store
            // writes into whatever the borrow names.
            ExprKind::Deref { expr: sub } => {
                let ptr = self.value_of(sub)?;
                self.g.builder.build_store(v, ptr);
                Ok(())
            }
            // The only other place a Kelp assignment can name is a chain this
            // slice cannot address, which the frontend does not produce.
            _ => Err(refuse("'assignment through a borrow'", "S2F")),
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

    /// `[a, b, c]` as one array *value*: the element array via `insertvalue`,
    /// then the wrapper struct's single field.
    fn emit_array_lit(
        &mut self,
        elems: &[Expr],
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let Ty::Array(elem, len) = ty else {
            return Err(no_value("an array literal's type"));
        };
        let array_ty = self.g.cty(ty)?;
        let elem_ty = self.lty(elem)?;
        let arr_ty = self.g.ctx.array_type(elem_ty, *len);
        let mut arr = self.g.ctx.undef(arr_ty);
        for (index, e) in elems.iter().enumerate() {
            let v = self.value_of(e)?;
            arr = self.g.builder.build_insert_value(arr, v, index as u32, "elem");
        }
        let mut agg = self.g.ctx.undef(array_ty);
        agg = self.g.builder.build_insert_value(agg, arr, 0, "v");
        Ok(agg)
    }

    /// `[v; N]` through the generated `{array}__fill(v)` helper.
    fn emit_array_repeat(&mut self, value: &Expr, ty: &Ty) -> Result<Value<'a>, String> {
        let Ty::Array(elem, len) = ty else {
            return Err(no_value("a repeat literal's type"));
        };
        let v = self.value_of(value)?;
        let symbol = format!("{}__fill", crate::types::array_c_type(elem, *len));
        let (func, fty) = self.g.cfn(&symbol)?;
        Ok(self.g.call(func, fty, &[v], "fill"))
    }

    /// Evaluate each argument a capability check reads, once, into a value the
    /// check and the call share. A checked argument is a `str` or `&str`
    /// (`stdlib::is_str_like_ty`), which is a bare pointer with no ownership, so
    /// `value_of` is exactly what the call path does with it (`arg_value` and
    /// `rt_arg_value` both fall through to `value_of` for a `str`-like
    /// argument). This is the eval-once half of BUG-Kf7Rn: a check that read a
    /// fresh rendering of the expression could admit one value and the call
    /// pass another.
    fn prepare_checked_args(
        &mut self,
        args: &[Expr],
        plan: Option<&CapPlan>,
    ) -> Result<HashMap<usize, Value<'a>>, String> {
        let mut prepared: HashMap<usize, Value<'a>> = HashMap::new();
        let Some(plan) = plan else {
            return Ok(prepared);
        };
        for check in &plan.checks {
            for index in cap_check_indices(check) {
                if prepared.contains_key(&index) {
                    continue;
                }
                let value = self.value_of(&args[index])?;
                prepared.insert(index, value);
            }
        }
        Ok(prepared)
    }

    /// Emit the runtime allowlist checks for one call site, before the call:
    /// one `kelp_cap_check_*` per resource argument, each with the domain, the
    /// resource, the module's compiled-in pattern array and its count, the
    /// calling module and the `.kp` call site.
    ///
    /// This is `codegen_c.rs::cap_checked_call`'s statement expression without
    /// the statement: the same symbols, the same argument order, and the same
    /// strings, because the deny path's text is pinned by the tests and the two
    /// backends must produce it byte for byte.
    fn emit_cap_checks(
        &mut self,
        plan: Option<&CapPlan>,
        prepared: &HashMap<usize, Value<'a>>,
    ) -> Result<(), String> {
        use crate::stdlib::CapCheck;
        let Some(plan) = plan else {
            return Ok(());
        };
        let domain = self.string_global(&plan.domain)?;
        let module = self.string_global(&self.module)?;
        let at = self.string_global(&plan.at)?;
        let patterns = *self
            .g
            .cap_arrays
            .get(&(self.module.clone(), plan.domain.clone()))
            .ok_or_else(|| no_value("the allowlist of a gated call site"))?;
        let count = self.g.v_i64(plan.pattern_count as u64);
        for check in &plan.checks {
            let value = |index: usize| -> Result<Value<'a>, String> {
                prepared
                    .get(&index)
                    .copied()
                    .ok_or_else(|| no_value("a checked call argument"))
            };
            let (func, ty) = match check {
                CapCheck::Path(_) => self.g.cap_fn("kelp_cap_check_path")?,
                CapCheck::Name(_) => self.g.cap_fn("kelp_cap_check_name")?,
                CapCheck::Host(_) => self.g.cap_fn("kelp_cap_check_host")?,
                CapCheck::HostPort(..) => self.g.cap_fn("kelp_cap_check_hostport")?,
                CapCheck::Port(_) => self.g.cap_fn("kelp_cap_check_port")?,
            };
            let args: Vec<Value<'a>> = match *check {
                CapCheck::Path(i) | CapCheck::Name(i) | CapCheck::Host(i) => {
                    vec![domain, value(i)?, patterns, count, module, at]
                }
                CapCheck::HostPort(host, port) => {
                    vec![domain, value(host)?, value(port)?, patterns, count, module, at]
                }
                CapCheck::Port(i) => vec![domain, value(i)?, patterns, count, module, at],
            };
            self.g.builder.build_call(func, ty, &args, "");
        }
        Ok(())
    }

    fn emit_call(
        &mut self,
        callee: &str,
        args: &[Expr],
        ret: &Ty,
        span: Span,
    ) -> Result<Option<Value<'a>>, String> {
        // A gated call site the C backend would guard with a runtime allowlist
        // check is checked here the same way: `cap_plan` decides the domain,
        // the arguments matched and the pattern count, and every resource
        // argument is evaluated once into a value the check and the call share
        // (BUG-Kf7Rn: evaluating it twice admits one value and passes another).
        let plan = self.g.cap_plan(&self.module, callee, args, span);
        let prepared = self.prepare_checked_args(args, plan.as_ref())?;

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
            for (index, a) in args.iter().enumerate() {
                match prepared.get(&index) {
                    Some(v) => values.push(*v),
                    None => values.push(self.arg_value(a, &mut held)?),
                }
            }
            // The allowlist checks run before the call, after every argument has
            // been evaluated: the statement expression `cap_checked_call` builds
            // has the same order.
            self.emit_cap_checks(plan.as_ref(), &prepared)?;
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
                for (index, a) in args.iter().enumerate() {
                    match prepared.get(&index) {
                        Some(v) => values.push(*v),
                        None => values.push(self.rt_arg_value(a, &mut held)?),
                    }
                }
                self.emit_cap_checks(plan.as_ref(), &prepared)?;
                self.g.builder.build_call(rt.func, rt.ty, &values, "");
                for (slot, ty) in held {
                    self.g.call_drop(slot, &ty)?;
                }
                return Ok(Some(self.g.builder.build_load(ret_ty, slot, "sret.val")));
            }
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
            for (index, a) in args.iter().enumerate() {
                match prepared.get(&index) {
                    Some(v) => values.push(*v),
                    None => values.push(self.rt_arg_value(a, &mut held)?),
                }
            }
            self.emit_cap_checks(plan.as_ref(), &prepared)?;
            let name = if rt.yields { "call" } else { "" };
            let v = self.g.builder.build_call(rt.func, rt.ty, &values, name);
            for (slot, ty) in held {
                self.g.call_drop(slot, &ty)?;
            }
            return Ok(if rt.yields { Some(v) } else { None });
        }

        // A builtin container method: the monomorphized function this slice
        // generated into the module, named exactly as the C backend names it.
        if let Some(symbol) = container_symbol(callee, args, ret) {
            let Some((func, fty)) = self.g.container_fns.get(&symbol).copied() else {
                return Err(refuse(&format!("the call to '{callee}'"), "S2D"));
            };
            let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
            for (index, a) in args.iter().enumerate() {
                match prepared.get(&index) {
                    Some(v) => values.push(*v),
                    None => values.push(self.arg_value(a, &mut held)?),
                }
            }
            self.emit_cap_checks(plan.as_ref(), &prepared)?;
            let yields_value = *ret != Ty::Unit;
            let name = if yields_value { "call" } else { "" };
            let v = self.g.builder.build_call(func, fty, &values, name);
            for (slot, ty) in held {
                self.g.call_drop(slot, &ty)?;
            }
            return Ok(if yields_value { Some(v) } else { None });
        }

        // Anything else is outside this slice. An `extern fun` is the runtime
        // and is S3; any other name is a construct this stage has no lowering
        // for.
        let stage = if self
            .g
            .program
            .functions
            .iter()
            .any(|f| f.name == callee && f.extern_)
        {
            "S3"
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
        let (spec, mut values, holds) = self.printf_piece(arg)?;
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
        for (slot, ty) in holds {
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
                    let (spec, mut values, piece_holds) = self.printf_piece(value)?;
                    fmt.push_str(&spec);
                    args.append(&mut values);
                    holds.extend(piece_holds);
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
                // A borrow of a field of an owned temporary (`panic(mk().msg)`)
                // stages the temporary the way every other borrow argument
                // does. Nothing is freed afterwards: a panic does not return, so
                // the drop would be unreachable (`codegen_c.rs` leaves it out for
                // the same reason).
                let ptr = self.borrow_place(arg, &mut Vec::new())?;
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
    ) -> Result<Option<(Value<'a>, Value<'a>, Vec<(Value<'a>, Ty)>)>, String> {
        match &arg.ty {
            Ty::Bytes | Ty::String => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "buf.tmp");
                self.g.builder.build_store(v, slot);
                let (data, len) = self.buffer_pair(slot, lty);
                Ok(Some((data, len, vec![(slot, arg.ty.clone())])))
            }
            Ty::View => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "view.tmp");
                self.g.builder.build_store(v, slot);
                let (data, len) = self.buffer_pair(slot, lty);
                Ok(Some((data, len, Vec::new())))
            }
            Ty::Ref { inner, .. }
                if matches!(**inner, Ty::Bytes | Ty::String | Ty::View) =>
            {
                let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
                let ptr = self.borrow_place(arg, &mut held)?;
                let lty = self.lty(inner)?;
                let (data, len) = self.buffer_pair(ptr, lty);
                Ok(Some((data, len, held)))
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
    ) -> Result<(String, Vec<Value<'a>>, Vec<(Value<'a>, Ty)>), String> {
        if let Some((data, len, holds)) = self.buffer_fields(arg)? {
            let len = self
                .g
                .builder
                .build_int_cast(false, false, len, self.g.ctx.i32(), "len");
            return Ok(("%.*s".to_string(), vec![len, data], holds));
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
                return Ok(("%u".to_string(), vec![promoted], Vec::new()));
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
                return Ok(("%f".to_string(), vec![promoted], Vec::new()));
            }
            Ty::F64 => "%f",
            Ty::Bool => {
                let t = self.string_global("true")?;
                let f = self.string_global("false")?;
                let selected = self.g.builder.build_select(v, t, f, "bool");
                return Ok(("%s".to_string(), vec![selected], Vec::new()));
            }
            Ty::Char => "%c",
            Ty::Str => "%s",
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => "%s",
            other => return Err(refuse_type(other)),
        };
        Ok((spec.to_string(), vec![v], Vec::new()))
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

// ===========================================================================
// The containers (MK7 S2D): `List<T>`, `Map<K, V>`, `Box<T>` and `[T; N]`.
//
// MK7 §3.2 names the exception this section exists for: the fixed runtime stays
// C, but the monomorphized container code cannot, because `codegen_c.rs`
// generates one function set per element or value type. So this backend
// generates the same sets into the module, one per instantiation, with the same
// function names and the same bodies the C backend writes. A container's header
// is a named struct whose field order and widths match the C typedefs field for
// field, but it never crosses to C by value: the runtime helpers it calls take
// a pointer (MK7 §3.3 and BUG-vN4tR).
// ===========================================================================

/// The `(name, each field's type)` pairs of every declaration, for the derived
/// clone/order fixpoints. The mirror of `codegen_c::type_decls`.
fn type_decls(
    structs: &[crate::types::StructDef],
    enums: &[crate::types::EnumDef],
) -> Vec<(String, Vec<Ty>)> {
    let enum_names: HashSet<&str> = enums.iter().map(|e| e.name.as_str()).collect();
    let mut out: Vec<(String, Vec<Ty>)> = Vec::new();
    for s in structs {
        if enum_names.contains(s.name.as_str()) {
            continue;
        }
        out.push((s.name.clone(), s.fields.iter().map(|f| f.ty.clone()).collect()));
    }
    for e in enums {
        out.push((
            e.name.clone(),
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                .collect(),
        ));
    }
    out
}

/// Every container instantiation a program mentions, the mirror of
/// `codegen_c::collect_containers`. Over-collecting is harmless (an unused set
/// is unused code), under-collecting is not, so every expression's own type and
/// every declaration's fields are noted, and a `Map` also notes the `List<K>`
/// and `List<V>` its `keys`/`values` return.
fn collect_instantiations(program: &Program) -> Instantiations {
    fn note_ty(ty: &Ty, out: &mut Instantiations) {
        match ty {
            Ty::List(elem) => {
                if **elem != Ty::Unit {
                    out.lists.insert((**elem).clone());
                }
                note_ty(elem, out);
            }
            Ty::Box(elem) => {
                if **elem != Ty::Unit {
                    out.boxes.insert((**elem).clone());
                }
                note_ty(elem, out);
            }
            Ty::Map(key, value) => {
                // An *open* map (`Map.new()` with no type anywhere yet) carries
                // `Unit` in a slot, which names no instantiation; the first
                // `insert` fills it in (`types::fill_open_container`).
                if **key == Ty::Unit || **value == Ty::Unit {
                    return;
                }
                // `keys()`/`values()` are generated with every map and return
                // `List<K>`/`List<V>`, so both instantiations exist even when
                // the program never calls them.
                out.maps.insert(((**key).clone(), (**value).clone()));
                out.lists.insert((**key).clone());
                out.lists.insert((**value).clone());
                note_ty(key, out);
                note_ty(value, out);
            }
            Ty::Array(elem, _) => {
                out.arrays.insert(ty.clone());
                note_ty(elem, out);
            }
            // A closure is emitted once per signature, and its parameters and
            // return type may name another closure or a container.
            Ty::Closure { params, ret } => {
                out.closures.insert(ty.clone());
                for p in params {
                    note_ty(p, out);
                }
                note_ty(ret, out);
            }
            Ty::Ref { inner, .. } => note_ty(inner, out),
            _ => {}
        }
    }
    fn note_block(block: &Block, out: &mut Instantiations) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { init, .. } => note_expr(init, out),
                Stmt::While { cond, body } => {
                    note_expr(cond, out);
                    note_block(body, out);
                }
                Stmt::Return { value, .. } => {
                    if let Some(e) = value {
                        note_expr(e, out);
                    }
                }
                Stmt::Expr(e) => note_expr(e, out),
            }
        }
        if let Some(tail) = &block.tail {
            note_expr(tail, out);
        }
    }
    fn note_expr(expr: &Expr, out: &mut Instantiations) {
        note_ty(&expr.ty, out);
        match &expr.kind {
            ExprKind::Unary { expr, .. }
            | ExprKind::Ref { expr, .. }
            | ExprKind::Deref { expr }
            | ExprKind::EnumTag { expr, .. }
            | ExprKind::Try { expr, .. }
            | ExprKind::ArrayRepeat { value: expr } => note_expr(expr, out),
            // A widened function value: the target signature needs the thunk
            // and its environment, and a value that *names* a function needs a
            // shim instead. The wrapped value needs the walk too.
            ExprKind::FnToClosure { func } => {
                if let ExprKind::FnRef(key) = &func.kind {
                    out.shims.insert((key.clone(), expr.ty.clone()));
                }
                out.coerced.insert(expr.ty.clone());
                note_expr(func, out);
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                note_expr(lhs, out);
                note_expr(rhs, out);
            }
            ExprKind::Assign { target, value } => {
                note_expr(target, out);
                note_expr(value, out);
            }
            ExprKind::Call { args, .. } | ExprKind::IndirectCall { args, .. } => {
                if let ExprKind::IndirectCall { callee, .. } = &expr.kind {
                    note_expr(callee, out);
                }
                for a in args {
                    note_expr(a, out);
                }
            }
            ExprKind::FieldAccess { base, .. }
            | ExprKind::EnumField { base, .. } => note_expr(base, out),
            ExprKind::StructLit { fields, .. } => {
                for (_, v) in fields {
                    note_expr(v, out);
                }
            }
            ExprKind::EnumLit { fields, .. } => {
                for f in fields {
                    note_expr(f, out);
                }
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                note_expr(cond, out);
                note_block(then_block, out);
                if let Some(b) = else_block {
                    note_block(b, out);
                }
            }
            ExprKind::Block(block) => note_block(block, out),
            ExprKind::ClosureLit { captures, .. } => {
                for c in captures {
                    note_expr(c, out);
                }
            }
            ExprKind::Template { pieces, .. } => {
                for p in pieces {
                    if let TemplatePiece::Value(v) = p {
                        note_expr(v, out);
                    }
                }
            }
            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    note_expr(e, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Instantiations::default();
    for s in &program.structs {
        for f in &s.fields {
            note_ty(&f.ty, &mut out);
        }
    }
    for e in &program.enums {
        for v in &e.variants {
            for f in &v.fields {
                note_ty(&f.ty, &mut out);
            }
        }
    }
    // A `pd` is a module-scope global with a drop of its own, so its declared
    // type needs its container function set even when nothing reads it: a
    // `pd val xs = List.of(7)` no expression calls `.get` on still drops at
    // exit, and the drop is a call into that set.
    for pd in &program.pds {
        note_ty(&pd.value_ty, &mut out);
        note_expr(&pd.init, &mut out);
    }
    for f in &program.functions {
        for (_, ty) in &f.params {
            note_ty(ty, &mut out);
        }
        note_ty(&f.ret, &mut out);
        // A literal's environment fields are values the generated function
        // holds, so their types need instantiations too (a captured `Box`, a
        // captured `String`).
        if let Some(env) = &f.env {
            for (_, ty) in &env.fields {
                note_ty(ty, &mut out);
            }
        }
        if !f.extern_ {
            note_block(&f.body, &mut out);
        }
    }
    out
}

/// Declare the libc primitives the container function sets reach, exactly as
/// the C backend reaches them through the standard headers. The signatures are
/// the ones `<stdlib.h>`/`<string.h>` declare, on the ABI this machine uses:
/// `size_t` is `i64` and `int` is `i32`.
/// The argument indices one capability check reads, in the order the check
/// reads them. `codegen_c.rs::checked_arg_flags` enumerates the same set; this
/// one keeps the order, which is what the two-argument `HostPort` check needs.
fn cap_check_indices(check: &crate::stdlib::CapCheck) -> Vec<usize> {
    use crate::stdlib::CapCheck;
    match *check {
        CapCheck::Path(i) | CapCheck::Name(i) | CapCheck::Host(i) | CapCheck::Port(i) => vec![i],
        CapCheck::HostPort(host, port) => vec![host, port],
    }
}

/// The symbol of the compiled-in allowlist array for one `(module, domain)`
/// grant, spelled exactly as `codegen_c.rs::CapTable::symbol` spells it: the
/// array is C data in the per-program object, and the IR declaration here has to
/// name the same symbol the object defines.
fn cap_array_symbol(module: &str, domain: &str) -> String {
    format!(
        "kelp_caps_{}_{}",
        crate::types::c_ident(&module.replace('/', "_")),
        domain
    )
}

/// Declare the capability check's C entry points, with the signatures
/// `runtime/cap_glue.c` defines.
///
/// The resource is a `const char*` for every check but the two that name a
/// port, where it is the `uint16_t` the grant is matched on. `count` is a
/// `size_t`, so `i64`. A declaration is free: the object only defines these when
/// the program contains a gated call, and an unreferenced declaration leaves no
/// undefined symbol behind.
fn declare_cap_fns<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i16t = ctx.i16();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let mut decl = |name: &'static str, params: &[Type<'a>]| {
        let ty = ctx.fn_type(void, params, false);
        let func = module.add_function(name, ty);
        out.insert(name, (func, ty));
    };
    let text = [ptr, ptr, ptr, i64t, ptr, ptr];
    decl("kelp_cap_check_path", &text);
    decl("kelp_cap_check_name", &text);
    decl("kelp_cap_check_host", &text);
    decl("kelp_cap_check_hostport", &[ptr, ptr, i16t, ptr, i64t, ptr, ptr]);
    decl("kelp_cap_check_port", &[ptr, i16t, ptr, i64t, ptr, ptr]);
    out
}

fn declare_libc<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i32t = ctx.i32();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let mut decl = |name: &'static str, ret: Type<'a>, params: &[Type<'a>]| {
        let ty = ctx.fn_type(ret, params, false);
        let func = module.add_function(name, ty);
        out.insert(name, (func, ty));
    };
    decl("malloc", ptr, &[i64t]);
    decl("realloc", ptr, &[ptr, i64t]);
    decl("free", void, &[ptr]);
    decl("memcpy", ptr, &[ptr, ptr, i64t]);
    decl("memcmp", i32t, &[ptr, ptr, i64t]);
    decl("memset", ptr, &[ptr, i32t, i64t]);
    decl("strlen", i64t, &[ptr]);
    // The one registration an owned `pd` has: its drop point is process exit
    // (`codegen_c.rs::write_pd_globals`), so the generated `kelp_init_pds`
    // hands `kelp_drop_pds` to `atexit`. Declared unconditionally like every
    // other libc symbol here; an unused declaration costs nothing.
    decl("atexit", i32t, &[ptr]);
    out
}

impl<'a> Gen<'a> {
    /// The pointer type LLVM 21 uses for every pointer.
    fn pty(&self) -> Type<'a> {
        self.ctx.opaque_ptr()
    }

    fn v_i64(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i64(), v, false)
    }

    fn v_i32(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i32(), v, false)
    }

    fn v_i8(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i8(), v, false)
    }

    /// The `elem`-typed address `index` elements past `ptr`, the plain
    /// non-`inbounds` GEP MK7 §3.4 wants for a data-dependent index.
    fn gep(&self, elem: Type<'a>, ptr: Value<'a>, index: Value<'a>, name: &str) -> Value<'a> {
        self.builder.build_gep(elem, ptr, index, name)
    }

    /// The address of field `idx` of the aggregate at `ptr`. The index is a
    /// compile-time constant inside an object of that exact type, which is the
    /// one place MK7 §3.4 allows the `inbounds` form.
    fn fld(&self, agg: Type<'a>, ptr: Value<'a>, idx: u32, name: &str) -> Value<'a> {
        self.builder.build_struct_gep(agg, ptr, idx, name)
    }

    fn ld(&self, ty: Type<'a>, ptr: Value<'a>, name: &str) -> Value<'a> {
        self.builder.build_load(ty, ptr, name)
    }

    fn st(&self, value: Value<'a>, ptr: Value<'a>) {
        self.builder.build_store(value, ptr);
    }

    fn cfn(&self, symbol: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.container_fns
            .get(symbol)
            .copied()
            .ok_or_else(|| no_value(&format!("the container function '{symbol}'")))
    }

    fn libc_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.libc
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the libc function '{name}'")) )
    }
    fn cap_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.cap_fns
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the capability check '{name}'")))
    }

    /// Append a block to `func` without moving the builder. A caller that
    /// creates several blocks before the terminator that branches to them
    /// needs exactly this: the terminator goes into the *current* block, and
    /// each destination is entered with an explicit `position_at_end`.
    fn block(&self, func: LlvmFunction<'a>, name: &str) -> BasicBlock<'a> {
        func.append_block(name)
    }

    /// A loop header: branch to a fresh block from wherever the builder is, and
    /// continue in it. This is the one place a block is created *and* entered,
    /// because a loop header is branched to once from the pre-header and once
    /// from the body.
    fn loop_header(&self, func: LlvmFunction<'a>, name: &str) -> BasicBlock<'a> {
        let bb = func.append_block(name);
        self.builder.build_br(bb);
        self.builder.position_at_end(bb);
        bb
    }

    /// Emit `kelp_panic(msg)` and terminate the block, for the bounds checks
    /// and the empty-container panics whose text must be byte-identical to the
    /// C backend's.
    fn emit_panic(&self, msg: &str) -> Result<(), String> {
        let ptr = self.builder.global_string(msg, "panic.msg");
        self.builder
            .build_call(self.panic_fn, self.panic_ty, &[ptr], "");
        self.builder.build_unreachable();
        Ok(())
    }

    fn call(&self, func: LlvmFunction<'a>, ty: FnType<'a>, args: &[Value<'a>], name: &str) -> Value<'a> {
        self.builder.build_call(func, ty, args, name)
    }

    // ---- container types -------------------------------------------------

    /// Create one named LLVM struct per container instantiation the program
    /// uses, with no body yet. The bodies are set later (`set_container_bodies`
    /// for the pointer containers, and alongside the aggregates for the inline
    /// arrays), because an array's body names the element type it holds.
    fn declare_container_types(&mut self) -> Result<(), String> {
        let inst = self.instantiations.clone();
        let mut lists: Vec<Ty> = inst.lists.iter().cloned().collect();
        lists.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        let mut boxes: Vec<Ty> = inst.boxes.iter().cloned().collect();
        boxes.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        let mut maps: Vec<(Ty, Ty)> = inst.maps.iter().cloned().collect();
        maps.sort_by_key(|(k, v)| format!("{}{}", container_sort_name(k), container_sort_name(v)));
        let mut arrays: Vec<Ty> = inst.arrays.iter().cloned().collect();
        arrays.sort_by_key(|t| (container_depth(t), container_sort_name(t)));

        // The closure signature types first (S2E): a container element may be
        // one (`Box<closure(...)>`), a parameter certainly may, and a later
        // closure signature may name an earlier one. The C backend emits these
        // before every container and `datac` for the same reason.
        let mut closures: Vec<Ty> = inst.closures.iter().cloned().collect();
        closures.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        for sig in &closures {
            let Ty::Closure { params, ret } = sig else {
                continue;
            };
            let ty = self.ctx.struct_type(&closure_c_type(params, ret));
            self.containers.insert(sig.clone(), ty);
        }

        for elem in &lists {
            let ty = self.ctx.struct_type(&list_c_type(elem));
            self.containers.insert(Ty::List(Box::new(elem.clone())), ty);
        }
        for elem in &boxes {
            let ty = self.ctx.struct_type(&box_c_type(elem));
            self.containers.insert(Ty::Box(Box::new(elem.clone())), ty);
        }
        for (key, value) in &maps {
            let name = map_c_type(key, value);
            let entry = self.ctx.struct_type(&format!("{name}_entry"));
            let ty = self.ctx.struct_type(&name);
            self.map_entries
                .insert(Ty::Map(Box::new(key.clone()), Box::new(value.clone())), entry);
            self.containers
                .insert(Ty::Map(Box::new(key.clone()), Box::new(value.clone())), ty);
        }
        for arr in &arrays {
            if let Ty::Array(elem, len) = arr {
                let ty = self.ctx.struct_type(&crate::types::array_c_type(elem, *len));
                self.containers.insert(arr.clone(), ty);
            }
        }
        // The pointer containers have no by-value dependency on a user type, so
        // their bodies are settable now, before any `datac` type exists.
        self.set_container_bodies()
    }

    /// Set the bodies of every pointer container (`List`, `Box`, `Map`). Safe to
    /// do before any `datac` body exists: a container holds its contents through
    /// a pointer, so none of these fields names a user type by value.
    fn set_container_bodies(&self) -> Result<(), String> {
        let ptr = self.pty();
        let i64t = self.ctx.i64();
        let i8t = self.ctx.i8();
        for (ty, lty) in &self.containers {
            match ty {
                Ty::List(_) => self.ctx.struct_set_body(*lty, &[ptr, i64t, i64t]),
                Ty::Box(_) => self.ctx.struct_set_body(*lty, &[ptr]),
                // The closure's fat pointer, field for field
                // `write_closure_typedef`'s three members (S2E): the code
                // pointer, the environment and the environment's destructor.
                Ty::Closure { .. } => self.ctx.struct_set_body(*lty, &[ptr, ptr, ptr]),
                Ty::Map(_, _) => self.ctx.struct_set_body(
                    *lty,
                    &[ptr, i64t, i64t, i64t, ptr, i64t, i64t],
                ),
                _ => {}
            }
        }
        for (map, entry_ty) in &self.map_entries {
            if let Ty::Map(key, _) = map {
                let key_ty = llvm_type(self.ctx, &self.structs, &self.containers, key)?;
                self.ctx.struct_set_body(*entry_ty, &[key_ty, ptr, i8t]);
            }
        }
        Ok(())
    }

    /// Set one array instantiation's body, after any array nested inside it
    /// (`emit_arrays_for`, the mirror of the C helper of the same name).
    fn emit_array_body(&self, ty: &Ty, emitted: &mut HashSet<Ty>) -> Result<(), String> {
        let Ty::Array(elem, len) = ty else {
            return Ok(());
        };
        self.emit_array_body(elem, emitted)?;
        if !emitted.insert(ty.clone()) {
            return Ok(());
        }
        let elem_ty = llvm_type(self.ctx, &self.structs, &self.containers, elem)?;
        let arr = self.ctx.array_type(elem_ty, *len);
        if let Some(lty) = self.containers.get(ty) {
            self.ctx.struct_set_body(*lty, &[arr]);
        }
        Ok(())
    }

    /// The libc call `malloc(sizeof(<ty>))` lowered to a constant byte count.
    fn malloc_for(&self, ty: Type<'a>) -> Result<Value<'a>, String> {
        let size = self.layout.abi_size_of(ty);
        let (func, fty) = self.libc_fn("malloc")?;
        Ok(self.call(func, fty, &[self.v_i64(size)], "heap"))
    }

    /// `kelp_panic("out of memory")` when `ptr` is NULL, the same check every
    /// generated C allocation makes. The block control continues in is returned,
    /// because a caller that tracks its own current block (the function-body
    /// walk) has to be told where the branch landed; the container bodies, which
    /// position the builder by hand, ignore it.
    fn check_alloc(&self, ptr: Value<'a>, func: LlvmFunction<'a>) -> Result<BasicBlock<'a>, String> {
        let null = self.ctx.const_null(self.pty());
        let isnull = self.builder.build_icmp(IntCmp::Eq, ptr, null, "oom");
        let oom = self.block(func, "oom");
        let ok = self.block(func, "alloc.ok");
        self.builder.build_cond_br(isnull, oom, ok);
        self.builder.position_at_end(oom);
        self.emit_panic("out of memory")?;
        self.builder.position_at_end(ok);
        Ok(ok)
    }

}

// ===========================================================================
// The closures (MK7 S2E): function values and the fat pointer a capturing
// `fun` literal becomes. The C backend's shape is mirrored member for member
// (see `codegen_c.rs::write_closure_typedef` and its neighbours): the value is
// `{ fn, env, drop }`, the code pointer takes the environment first, the
// environment and the widening's one-field wrapper are heap, and the only
// function in a signature's own set is the drop. Per-instantiation code is
// generated into the module, which is MK7 §3.2's one exception to "the runtime
// stays C".
// ===========================================================================

impl<'a> Gen<'a> {
    /// The LLVM types of one closure signature: its own parameters and its
    /// return, with `unit` becoming `void`. The fat pointer's own struct type is
    /// in `containers`, keyed by the `tir` closure type.
    fn closure_sig_types(&self, ty: &Ty) -> Result<(Vec<Type<'a>>, Type<'a>), String> {
        let Ty::Closure { params, ret } = ty else {
            return Err(no_value("a closure signature"));
        };
        let params = params
            .iter()
            .map(|p| self.lty2(p))
            .collect::<Result<Vec<_>, _>>()?;
        let ret = if **ret == Ty::Unit {
            self.ctx.void()
        } else {
            self.lty2(ret)?
        };
        Ok((params, ret))
    }

    /// The code-pointer type of one closure signature: the environment first,
    /// then the closure's own parameters. The `fn` member, the widening thunk
    /// and the shim are all declared with it.
    fn closure_code_ty(&self, ty: &Ty) -> Result<FnType<'a>, String> {
        let (params, ret) = self.closure_sig_types(ty)?;
        let mut all = Vec::with_capacity(params.len() + 1);
        all.push(self.pty());
        all.extend(params);
        Ok(self.ctx.fn_type(ret, &all, false))
    }

    /// The struct type of a closure value, looking through a borrow: a
    /// `Ty::Ref` of a closure is the pointer a parameter of one takes, so the
    /// struct is the pointee's.
    fn closure_value_ty(&self, ty: &Ty) -> Result<Type<'a>, String> {
        match ty {
            Ty::Ref { inner, .. } => self.closure_value_ty(inner),
            Ty::Closure { .. } => self.cty(ty),
            other => Err(refuse_type(other)),
        }
    }

    /// The `(params, ret)` LLVM types of a plain function value, looking
    /// through a borrow. `unit` becomes `void`.
    fn fn_value_sig(&self, ty: &Ty) -> Result<(Vec<Type<'a>>, Type<'a>), String> {
        let inner = match ty {
            Ty::Ref { inner, .. } => inner.as_ref(),
            other => other,
        };
        let Ty::Fn { params, ret } = inner else {
            return Err(no_value("a function value"));
        };
        let params = params
            .iter()
            .map(|p| self.lty2(p))
            .collect::<Result<Vec<_>, _>>()?;
        let ret = if **ret == Ty::Unit {
            self.ctx.void()
        } else {
            self.lty2(ret)?
        };
        Ok((params, ret))
    }

    /// One generated S2E function by its symbol, so the lowering can take its
    /// address or call it. Every symbol in the map was declared in
    /// `declare_closure_fns`, before any body that names one.
    fn closure_symbol(&self, symbol: &str) -> Result<LlvmFunction<'a>, String> {
        self.closure_fns
            .get(symbol)
            .map(|(func, _)| *func)
            .ok_or_else(|| no_value(&format!("the closure function '{symbol}'")))
    }

    /// The environment struct of every capturing literal in the program: one
    /// field per capture, in capture order, the layout `write_env` gives the C
    /// struct. Emitted after every aggregate type exists, so a captured
    /// `datac` names a complete type, and before any signature, so the hidden
    /// `ptr` it is passed as needs nothing else.
    fn declare_closure_envs(&mut self) -> Result<(), String> {
        for f in &self.program.functions {
            let Some(env) = &f.env else {
                continue;
            };
            let fields = env
                .fields
                .iter()
                .map(|(_, ty)| llvm_type(self.ctx, &self.structs, &self.containers, ty))
                .collect::<Result<Vec<_>, _>>()?;
            let ty = self.ctx.struct_type(&env.struct_name);
            self.ctx.struct_set_body(ty, &fields);
            self.env_types.insert(f.name.clone(), ty);
        }
        Ok(())
    }

    /// Declare the S2E machinery: the per-signature drop, one environment
    /// destructor per capturing literal, the widening thunk and its
    /// environment's destructor, and the shim a known function value is widened
    /// through. One name each, the same names the C backend gives them, so the
    /// IR reads like the generated C beside it.
    fn declare_closure_fns(&mut self) -> Result<(), String> {
        let inst = self.instantiations.clone();
        let ptr = self.pty();
        let void = self.ctx.void();

        let mut closures: Vec<Ty> = inst.closures.iter().cloned().collect();
        closures.sort_by_key(container_sort_name);
        for sig in &closures {
            let symbol = drop_fn_name(sig);
            let ty = self.ctx.fn_type(void, &[ptr], false);
            let func = self.module.add_function(&symbol, ty);
            self.closure_fns.insert(symbol, (func, ty));
        }

        // One destructor per capturing literal (S2E): the function the closure
        // value carries in its `drop` member, which frees the environment the
        // captures were moved into. It is a separate function from the
        // per-signature closure drop because the environment's layout belongs
        // to the one literal that made it, exactly as in `write_env`.
        for f in &self.program.functions {
            let Some(env) = &f.env else {
                continue;
            };
            let ty = self.ctx.fn_type(void, &[ptr], false);
            let func = self.module.add_function(&env.drop_name, ty);
            self.closure_fns.insert(env.drop_name.clone(), (func, ty));
        }

        let mut coerced: Vec<Ty> = inst.coerced.iter().cloned().collect();
        coerced.sort_by_key(container_sort_name);
        for sig in &coerced {
            let code_ty = self.closure_code_ty(sig)?;
            let thunk = coerce_thunk_symbol(sig);
            let func = self.module.add_function(&thunk, code_ty);
            self.closure_fns.insert(thunk, (func, code_ty));
            let drop = coerce_drop_symbol(sig);
            let dty = self.ctx.fn_type(void, &[ptr], false);
            let dfunc = self.module.add_function(&drop, dty);
            self.closure_fns.insert(drop, (dfunc, dty));
        }

        let mut shims: Vec<(String, Ty)> = inst.shims.iter().cloned().collect();
        shims.sort_by_key(|(key, _)| key.clone());
        for (key, sig) in &shims {
            let code_ty = self.closure_code_ty(sig)?;
            let symbol = shim_symbol(key, sig);
            let func = self.module.add_function(&symbol, code_ty);
            self.closure_fns.insert(symbol, (func, code_ty));
        }
        Ok(())
    }

    /// Define every function `declare_closure_fns` declared, in the order the C
    /// backend writes them: the per-signature drop, then the widening's
    /// machinery, then the shims.
    fn emit_closure_bodies(&self) -> Result<(), String> {
        let inst = self.instantiations.clone();
        // Each capturing literal's environment destructor first, because the
        // closure values built below carry its address.
        for f in &self.program.functions {
            if f.env.is_some() {
                self.emit_env_drop(f)?;
            }
        }
        let mut closures: Vec<Ty> = inst.closures.iter().cloned().collect();
        closures.sort_by_key(container_sort_name);
        for sig in &closures {
            self.emit_closure_drop(sig)?;
        }
        let mut coerced: Vec<Ty> = inst.coerced.iter().cloned().collect();
        coerced.sort_by_key(container_sort_name);
        for sig in &coerced {
            self.emit_coerce_drop(sig)?;
            self.emit_coerce_thunk(sig)?;
        }
        let mut shims: Vec<(String, Ty)> = inst.shims.iter().cloned().collect();
        shims.sort_by_key(|(key, _)| key.clone());
        for (key, sig) in &shims {
            self.emit_shim(key, sig)?;
        }
        Ok(())
    }

    /// One capturing literal's environment destructor (S2E): drop each owned
    /// field through its own drop, then free the struct. The same order a
    /// `datac`'s drop uses, one level down (`write_env`).
    fn emit_env_drop(&self, f: &Function) -> Result<(), String> {
        let env = f
            .env
            .as_ref()
            .ok_or_else(|| no_value("the environment destructor of a plain function"))?;
        let (func, _) = *self
            .closure_fns
            .get(&env.drop_name)
            .ok_or_else(|| no_value(&format!("the environment destructor '{}'", env.drop_name)))?;
        let env_ty = *self
            .env_types
            .get(&f.name)
            .ok_or_else(|| no_value(&format!("the environment of '{}'", f.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        for (index, (name, ty)) in env.fields.iter().enumerate() {
            if !needs_drop(ty, &self.owned) {
                continue;
            }
            let field = self.builder.build_struct_gep(env_ty, raw, index as u32, name);
            self.call_drop(field, ty)?;
        }
        let (free, fty) = self.libc_fn("free")?;
        self.call(free, fty, &[raw], "");
        self.builder.build_ret_void();
        Ok(())
    }

    /// One closure signature's drop, the only function in its set: free the
    /// environment through the destructor the value carries, then clear the
    /// pointer. The `env` test comes first because a value widened from a plain
    /// function has no environment, and its destructor pointer is `null` too
    /// (`write_closure` draws the same line).
    fn emit_closure_drop(&self, sig: &Ty) -> Result<(), String> {
        let symbol = drop_fn_name(sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the closure drop '{symbol}'")))?;
        let closure_ty = self.closure_value_ty(sig)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let c = func.get_param(0);
        let env_slot = self.fld(closure_ty, c, 1, "env");
        let env = self.ld(ptr, env_slot, "env");
        let null = self.ctx.const_null(ptr);
        let missing = self.builder.build_icmp(IntCmp::Eq, env, null, "no.env");
        let exit = func.append_block("exit");
        let live = func.append_block("drop");
        self.builder.build_cond_br(missing, exit, live);
        self.builder.position_at_end(live);
        let drop = self.ld(ptr, self.fld(closure_ty, c, 2, "drop"), "drop");
        let dty = self.ctx.fn_type(void, &[ptr], false);
        self.builder.build_indirect_call(drop, dty, &[env], "");
        self.st(null, env_slot);
        self.builder.build_br(exit);
        self.builder.position_at_end(exit);
        self.builder.build_ret_void();
        Ok(())
    }

    /// The destructor of a widened function value's one-field environment: a
    /// plain `free`, because the code pointer it holds owns nothing.
    fn emit_coerce_drop(&self, sig: &Ty) -> Result<(), String> {
        let symbol = coerce_drop_symbol(sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the coerce drop '{symbol}'")))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        let (free, fty) = self.libc_fn("free")?;
        self.call(free, fty, &[raw], "");
        self.builder.build_ret_void();
        Ok(())
    }

    /// The thunk of a widened function value: read the code pointer back out of
    /// the environment and call through it. One thunk per *signature*, because
    /// what it does is the same for every value of that shape.
    fn emit_coerce_thunk(&self, sig: &Ty) -> Result<(), String> {
        let symbol = coerce_thunk_symbol(sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the coerce thunk '{symbol}'")))?;
        let (params, ret) = self.closure_sig_types(sig)?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        let f = self.ld(self.pty(), self.fld(self.coerce_env_ty, raw, 0, "coerce.f"), "f");
        let args: Vec<Value<'a>> = (0..params.len()).map(|i| func.get_param(i + 1)).collect();
        let call_ty = self.ctx.fn_type(ret, &params, false);
        if closure_is_unit(sig) {
            self.builder.build_indirect_call(f, call_ty, &args, "");
            self.builder.build_ret_void();
        } else {
            let v = self.builder.build_indirect_call(f, call_ty, &args, "call");
            self.builder.build_ret(v);
        }
        Ok(())
    }

    /// The shim of a widened *known* function value: take the environment a
    /// closure's `fn` member is called with, ignore it, and call the function it
    /// wraps. A named function needs no environment and no indirection, so the
    /// common widening allocates nothing.
    fn emit_shim(&self, key: &str, sig: &Ty) -> Result<(), String> {
        let symbol = shim_symbol(key, sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the shim '{symbol}'")))?;
        let (params, _) = self.closure_sig_types(sig)?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let callee = self
            .funcs
            .get(key)
            .cloned()
            .ok_or_else(|| refuse(&format!("the function value '{key}'"), "S3"))?;
        let args: Vec<Value<'a>> = (0..params.len()).map(|i| func.get_param(i + 1)).collect();
        if closure_is_unit(sig) {
            self.call(callee.func, callee.ty, &args, "");
            self.builder.build_ret_void();
        } else {
            let v = self.call(callee.func, callee.ty, &args, "call");
            self.builder.build_ret(v);
        }
        Ok(())
    }
}

/// The mangled signature fragment of a closure type, for the generated S2E
/// symbols. The value is always a `Ty::Closure`; a `Ty::Fn` has no environment
/// and so no machinery of its own.
fn closure_mangle_of(ty: &Ty) -> String {
    match ty {
        Ty::Closure { params, ret } => closure_mangle(params, ret),
        other => unreachable!("not a closure signature: {}", other.display()),
    }
}

/// The generated names of the widening's two halves and of the shim, mirroring
/// `codegen_c.rs::coerce_thunk_name`/`coerce_drop_name`/`coerce_shim_name`.
fn coerce_thunk_symbol(ty: &Ty) -> String {
    format!("kelp_coerce__{}", closure_mangle_of(ty))
}

fn coerce_drop_symbol(ty: &Ty) -> String {
    format!("kelp_coerce_drop__{}", closure_mangle_of(ty))
}

fn shim_symbol(key: &str, ty: &Ty) -> String {
    format!("kelp_shim__{}__{}", crate::types::c_ident(key), closure_mangle_of(ty))
}

/// The signature a closure callee is called with, when the callee is a closure
/// value or a borrow of one. `None` for a plain function value, whose callee is
/// the pointer itself.
fn closure_callee_sig(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Closure { .. } => Some(ty.clone()),
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Closure { .. }) => Some((**inner).clone()),
        _ => None,
    }
}

/// Whether one closure signature returns `unit`, which decides between `ret`
/// and `ret void` in the three functions that are declared with it.
fn closure_is_unit(sig: &Ty) -> bool {
    matches!(sig, Ty::Closure { ret, .. } if **ret == Ty::Unit)
}

/// How deeply containers nest, used to emit a container after the containers it
/// contains. The mirror of `codegen_c::container_depth`.
fn container_depth(ty: &Ty) -> usize {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => 1 + container_depth(elem),
        Ty::Map(..) => 1,
        Ty::Array(elem, _) => 1 + container_depth(elem),
        _ => 0,
    }
}

/// A stable sort key for the emission order: the C name of one element type.
fn container_sort_name(ty: &Ty) -> String {
    ty_mangle(ty)
}

/// One monomorphized container function: its mangled symbol, its LLVM return
/// type and its LLVM parameter types. What `declare_container_fns` declares and
/// `emit_container_bodies` defines, from one list, so the two cannot drift.
struct CfnSpec<'a> {
    symbol: String,
    ret: Type<'a>,
    params: Vec<Type<'a>>,
}

impl<'a> Gen<'a> {
    fn lty2(&self, ty: &Ty) -> Result<Type<'a>, String> {
        llvm_type(self.ctx, &self.structs, &self.containers, ty)
    }

    /// The named LLVM type of one container instantiation.
    fn cty(&self, ty: &Ty) -> Result<Type<'a>, String> {
        self.containers
            .get(ty)
            .copied()
            .ok_or_else(|| no_value(&format!("the container '{}'", ty.display())))
    }

    /// Position the builder at a fresh block of the named container function.
    fn start(
        &self,
        symbol: &str,
        name: &str,
    ) -> Result<(LlvmFunction<'a>, BasicBlock<'a>), String> {
        let (func, _) = self.cfn(symbol)?;
        let bb = func.append_block(name);
        self.builder.position_at_end(bb);
        Ok((func, bb))
    }

    /// One runtime helper by symbol, with its declared type.
    fn rt(&self, symbol: &str) -> Result<RtFn<'a>, String> {
        self.runtime
            .get(symbol)
            .copied()
            .ok_or_else(|| no_value(&format!("the runtime '{symbol}'")))
    }

    /// Call an sret runtime helper: the destination pointer first, then the
    /// real arguments (`RtFn::sret`).
    fn sret_call(&self, symbol: &str, dst: Value<'a>, args: &[Value<'a>]) -> Result<(), String> {
        let rt = self.rt(symbol)?;
        let mut all = Vec::with_capacity(args.len() + 1);
        all.push(dst);
        all.extend_from_slice(args);
        self.call(rt.func, rt.ty, &all, "");
        Ok(())
    }

    /// Deep-copy one element value (a `String`/`Bytes` clone, a `datac`'s
    /// generated clone, or a plain copy) into caller storage that owns nothing
    /// yet. The mirror of `codegen_c::write_field_clone` for a container slot.
    fn clone_into(&self, src: Value<'a>, dst: Value<'a>, elem: &Ty) -> Result<(), String> {
        match elem {
            Ty::String => self.sret_call("kelp_String__clone", dst, &[src]),
            Ty::Bytes => self.sret_call("kelp_Bytes__clone", dst, &[src]),
            Ty::Struct(name) => {
                let (func, ty) = self
                    .clone_fns
                    .get(name)
                    .copied()
                    .ok_or_else(|| no_value(&format!("the clone of '{name}'")))?;
                self.call(func, ty, &[src, dst], "");
                Ok(())
            }
            _ => {
                let elem_ty = self.lty2(elem)?;
                let v = self.ld(elem_ty, src, "copy");
                self.st(v, dst);
                Ok(())
            }
        }
    }

    /// The value of a copied element (a scalar load), for a `Map` value. Only
    /// reached for a `Copy` element or a `String`/`Bytes` clone, because
    /// `list_clonable_elem` refuses a `datac`.
    fn clone_value(&self, src: Value<'a>, elem: &Ty, slot: Value<'a>) -> Result<Value<'a>, String> {
        match elem {
            Ty::String => {
                self.sret_call("kelp_String__clone", slot, &[src])?;
                Ok(self.ld(self.lty2(elem)?, slot, "clone"))
            }
            Ty::Bytes => {
                self.sret_call("kelp_Bytes__clone", slot, &[src])?;
                Ok(self.ld(self.lty2(elem)?, slot, "clone"))
            }
            _ => Ok(self.ld(self.lty2(elem)?, src, "copy")),
        }
    }

    /// Widen a scalar map key to the `uint64_t` `kelp_map_mix` takes, by the
    /// key's own signedness, exactly as the C `(uint64_t)(k)` cast does.
    fn key_to_u64(&self, key: &Ty, v: Value<'a>) -> Result<Value<'a>, String> {
        let w = int_width(key).ok_or_else(|| no_value("a scalar map key"))?;
        if w == 64 {
            return Ok(v);
        }
        Ok(self
            .builder
            .build_int_cast(is_signed(key), true, v, self.ctx.i64(), "kw"))
    }

    /// The generated clone/cmp function handles, then the container sets.
    fn declare_clone_cmp_fns(&mut self) {
        let ptr = self.pty();
        let void = self.ctx.void();
        let i32t = self.ctx.i32();
        let mut clonable: Vec<String> = self.clonable.iter().cloned().collect();
        clonable.sort();
        for name in clonable {
            let ty = self.ctx.fn_type(void, &[ptr, ptr], false);
            let func = self
                .module
                .add_function(&clone_fn_name(&Ty::Struct(name.clone())), ty);
            self.clone_fns.insert(name, (func, ty));
        }
        let mut sortable: Vec<String> = self.sortable.iter().cloned().collect();
        sortable.sort();
        for name in sortable {
            let ty = self.ctx.fn_type(i32t, &[ptr, ptr], false);
            let func = self
                .module
                .add_function(&cmp_fn_name(&Ty::Struct(name.clone())), ty);
            self.cmp_fns.insert(name, (func, ty));
        }
    }

    /// The one list of functions each container kind is emitted with, gathered
    /// before any body so a callee is always declared first.
    fn list_specs(&self, elem: &Ty) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = list_c_type(elem);
        let list = self.cty(&Ty::List(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let (i1t, i32t, i64t) = (self.ctx.i1(), self.ctx.i32(), self.ctx.i64());
        let sortable = crate::types::value_is_sortable(elem, &self.sortable);
        let clonable = crate::types::value_is_clonable(elem, &self.clonable);
        let comparable = crate::types::list_comparable_elem(elem);
        let mut out = vec![
            spec(&format!("{base}__empty"), list, &[]),
            spec(&format!("{base}__reserve"), void, &[ptr, i64t]),
            spec(&format!("{base}__of"), list, &[elem_ty]),
            spec(&format!("{base}__len"), i64t, &[ptr]),
            spec(&format!("{base}__is_empty"), i1t, &[ptr]),
            spec(&format!("{base}__push"), void, &[ptr, elem_ty]),
            spec(&format!("{base}__bounds"), void, &[ptr, i64t]),
        ];
        if elem.is_copy() {
            out.push(spec(&format!("{base}__get"), elem_ty, &[ptr, i64t]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr, i64t]));
        }
        out.push(spec(&format!("{base}__set"), void, &[ptr, i64t, elem_ty]));
        out.push(spec(&format!("{base}__pop"), elem_ty, &[ptr]));
        out.push(spec(&format!("{base}__clear"), void, &[ptr]));
        out.push(spec(&format!("{base}__reverse"), void, &[ptr]));
        if sortable {
            out.push(spec(&format!("{base}__cmp"), i32t, &[ptr, ptr]));
            out.push(spec(&format!("{base}__sort"), void, &[ptr]));
        }
        // `contains` is gated on equality, which is a wider set than order in
        // the C backend only for a `datac` element: `list_comparable_elem` is
        // the same set as `list_sortable_elem`, so a `datac` element gets
        // `sort` but never `contains`.
        if comparable {
            // A `String`/`Bytes` element is compared through a shared borrow
            // (`contains_param`), every other comparable element by value.
            let contains_param = if elem.is_copy() { elem_ty } else { ptr };
            out.push(spec(&format!("{base}__contains"), i1t, &[ptr, contains_param]));
        }
        if clonable {
            out.push(spec(&format!("{base}__slice"), list, &[ptr, i64t, i64t]));
            out.push(spec(&format!("{base}__clone"), list, &[ptr]));
        }
        out.push(spec(&format!("{base}__insert"), void, &[ptr, i64t, elem_ty]));
        out.push(spec(&format!("{base}__remove"), elem_ty, &[ptr, i64t]));
        out.push(spec(
            &drop_fn_name(&Ty::List(Box::new(elem.clone()))),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    fn box_specs(&self, elem: &Ty) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = box_c_type(elem);
        let boxed = self.cty(&Ty::Box(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let i1t = self.ctx.i1();
        let mut out = vec![
            spec(&format!("{base}__empty"), boxed, &[]),
            spec(&format!("{base}__of"), boxed, &[elem_ty]),
            spec(&format!("{base}__is_empty"), i1t, &[ptr]),
        ];
        if elem.is_copy() {
            out.push(spec(&format!("{base}__get"), elem_ty, &[ptr]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr]));
        }
        out.push(spec(&format!("{base}__set"), void, &[ptr, elem_ty]));
        out.push(spec(&format!("{base}__take"), elem_ty, &[ptr]));
        out.push(spec(
            &drop_fn_name(&Ty::Box(Box::new(elem.clone()))),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    fn array_specs(&self, elem: &Ty, len: u64) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = crate::types::array_c_type(elem, len);
        let array = self.cty(&Ty::Array(Box::new(elem.clone()), len))?;
        let elem_ty = self.lty2(elem)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let (i1t, i64t) = (self.ctx.i1(), self.ctx.i64());
        let mut out = vec![
            spec(&format!("{base}__len"), i64t, &[ptr]),
            spec(&format!("{base}__is_empty"), i1t, &[ptr]),
            spec(&format!("{base}__bounds"), void, &[i64t]),
        ];
        if elem.is_copy() {
            out.push(spec(&format!("{base}__get"), elem_ty, &[ptr, i64t]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr, i64t]));
        }
        out.push(spec(&format!("{base}__set"), void, &[ptr, i64t, elem_ty]));
        out.push(spec(&format!("{base}__fill"), array, &[elem_ty]));
        out.push(spec(
            &drop_fn_name(&Ty::Array(Box::new(elem.clone()), len)),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    fn declare_container_fns(&mut self) -> Result<(), String> {
        let mut specs: Vec<CfnSpec<'a>> = Vec::new();
        for elem in self.lists_sorted() {
            specs.extend(self.list_specs(&elem)?);
        }
        for elem in self.boxes_sorted() {
            specs.extend(self.box_specs(&elem)?);
        }
        for (key, value) in self.maps_sorted() {
            specs.extend(self.map_specs(&key, &value)?);
        }
        for arr in self.arrays_sorted() {
            if let Ty::Array(elem, len) = arr {
                specs.extend(self.array_specs(&elem, len)?);
            }
        }
        for s in specs {
            let ty = self.ctx.fn_type(s.ret, &s.params, false);
            let func = self.module.add_function(&s.symbol, ty);
            self.container_fns.insert(s.symbol, (func, ty));
        }
        Ok(())
    }

    fn lists_sorted(&self) -> Vec<Ty> {
        let mut v: Vec<Ty> = self.instantiations.lists.iter().cloned().collect();
        v.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        v
    }

    fn boxes_sorted(&self) -> Vec<Ty> {
        let mut v: Vec<Ty> = self.instantiations.boxes.iter().cloned().collect();
        v.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        v
    }

    fn maps_sorted(&self) -> Vec<(Ty, Ty)> {
        let mut v: Vec<(Ty, Ty)> = self.instantiations.maps.iter().cloned().collect();
        v.sort_by_key(|(k, val)| format!("{}{}", container_sort_name(k), container_sort_name(val)));
        v
    }

    fn arrays_sorted(&self) -> Vec<Ty> {
        let mut v: Vec<Ty> = self.instantiations.arrays.iter().cloned().collect();
        v.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        v
    }
}

/// One `CfnSpec` from its three parts.
fn spec<'a>(symbol: &str, ret: Type<'a>, params: &[Type<'a>]) -> CfnSpec<'a> {
    CfnSpec {
        symbol: symbol.to_string(),
        ret,
        params: params.to_vec(),
    }
}

/// The list's three fields, in the order `write_container_typedef` declares
/// them: the buffer, the length, the capacity.
const L_DATA: u32 = 0;
const L_LEN: u32 = 1;
const L_CAP: u32 = 2;

impl<'a> Gen<'a> {
    // ---- the derived clone and order of a datac/enum ---------------------

    fn emit_clone_cmp_bodies(&self) -> Result<(), String> {
        let ptr = self.pty();
        let _ = ptr;
        for s in &self.program.structs {
            let fields: Vec<(String, Ty)> =
                s.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
            self.emit_record_clone(&s.name, &fields)?;
            self.emit_record_cmp(&s.name, &fields)?;
        }
        for e in &self.program.enums {
            self.emit_enum_clone(e)?;
            self.emit_enum_cmp(e)?;
        }
        Ok(())
    }

    fn emit_record_clone(&self, name: &str, fields: &[(String, Ty)]) -> Result<(), String> {
        let Some((func, _)) = self.clone_fns.get(name).copied() else {
            return Ok(());
        };
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let struct_ty = *self
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let src = func.get_param(0);
        let dst = func.get_param(1);
        for (i, (_, ty)) in fields.iter().enumerate() {
            let s = self.fld(struct_ty, src, i as u32, "src");
            let d = self.fld(struct_ty, dst, i as u32, "dst");
            self.clone_into(s, d, ty)?;
        }
        self.builder.build_ret_void();
        Ok(())
    }

    fn emit_record_cmp(&self, name: &str, fields: &[(String, Ty)]) -> Result<(), String> {
        let Some((func, _)) = self.cmp_fns.get(name).copied() else {
            return Ok(());
        };
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let struct_ty = *self
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let a = func.get_param(0);
        let b = func.get_param(1);
        for (i, (_, ty)) in fields.iter().enumerate() {
            let ap = self.fld(struct_ty, a, i as u32, "a");
            let bp = self.fld(struct_ty, b, i as u32, "b");
            let cont = self.emit_field_cmp(func, ap, bp, ty)?;
            self.builder.position_at_end(cont);
        }
        self.builder.build_ret(self.v_i32(0));
        Ok(())
    }

    /// One field's contribution to a derived order, returning the block the walk
    /// continues in: the first differing field decides, exactly as
    /// `write_field_cmp` returns early on the first difference.
    fn emit_field_cmp(
        &self,
        func: LlvmFunction<'a>,
        ap: Value<'a>,
        bp: Value<'a>,
        ty: &Ty,
    ) -> Result<BasicBlock<'a>, String> {
        let ret_ty = self.lty2(ty)?;
        match ty {
            Ty::String | Ty::Bytes => {
                let (data_o, len_o) = (self.pty(), self.ctx.i64());
                let alp = self.fld(ret_ty, ap, 1, "alen");
                let blp = self.fld(ret_ty, bp, 1, "blen");
                let alen = self.ld(len_o, alp, "alen");
                let blen = self.ld(len_o, blp, "blen");
                let min = self.builder.build_select(
                    self.builder.build_icmp(IntCmp::Ult, alen, blen, "lt"),
                    alen,
                    blen,
                    "n",
                );
                // min > 0 ? memcmp(ap.data, bp.data, min) : 0
                let nonzero = self.builder.build_icmp(IntCmp::Ugt, min, self.v_i64(0), "nz");
                let cmp_bb = func.append_block("cmp.bytes");
                let after_cmp_bb = func.append_block("cmp.after");
                self.builder.build_cond_br(nonzero, cmp_bb, after_cmp_bb);
                self.builder.position_at_end(cmp_bb);
                let adata = self.ld(data_o, self.fld(ret_ty, ap, 0, "ad"), "ad");
                let bdata = self.ld(data_o, self.fld(ret_ty, bp, 0, "bd"), "bd");
                let (memcmp, memcmp_ty) = self.libc_fn("memcmp")?;
                let c = self.call(memcmp, memcmp_ty, &[adata, bdata, min], "memcmp");
                let ne = self.builder.build_icmp(IntCmp::Ne, c, self.v_i32(0), "ne");
                let diff_bb = func.append_block("cmp.diff");
                let keep_bb = func.append_block("cmp.keep");
                self.builder.build_cond_br(ne, diff_bb, keep_bb);
                self.builder.position_at_end(diff_bb);
                let sign = self.builder.build_select(
                    self.builder.build_icmp(IntCmp::Slt, c, self.v_i32(0), "neg"),
                    self.v_i32(u64::MAX),
                    self.v_i32(1),
                    "sign",
                );
                self.builder.build_ret(sign);
                self.builder.position_at_end(keep_bb);
                self.builder.build_br(after_cmp_bb);
                self.builder.position_at_end(after_cmp_bb);
                // Equal prefixes: the shorter buffer orders first.
                let lengths_differ =
                    self.builder.build_icmp(IntCmp::Ne, alen, blen, "lendiff");
                let len_bb = func.append_block("cmp.len");
                let cont = func.append_block("cmp.cont");
                self.builder.build_cond_br(lengths_differ, len_bb, cont);
                self.builder.position_at_end(len_bb);
                let lsign = self.builder.build_select(
                    self.builder.build_icmp(IntCmp::Ult, alen, blen, "alenlt"),
                    self.v_i32(u64::MAX),
                    self.v_i32(1),
                    "lsign",
                );
                self.builder.build_ret(lsign);
                self.builder.position_at_end(cont);
                Ok(cont)
            }
            Ty::Struct(name) => {
                let (func_c, ty_c) = self
                    .cmp_fns
                    .get(name)
                    .copied()
                    .ok_or_else(|| no_value(&format!("the order of '{name}'")))?;
                let c = self.call(func_c, ty_c, &[ap, bp], "cmp");
                let ne = self.builder.build_icmp(IntCmp::Ne, c, self.v_i32(0), "ne");
                let diff = func.append_block("cmp.diff");
                let cont = func.append_block("cmp.cont");
                self.builder.build_cond_br(ne, diff, cont);
                self.builder.position_at_end(diff);
                self.builder.build_ret(c);
                self.builder.position_at_end(cont);
                Ok(cont)
            }
            _ => {
                let av = self.ld(ret_ty, ap, "av");
                let bv = self.ld(ret_ty, bp, "bv");
                // C's `a != b` / `a < b` are `fcmp` for a float field, which is
                // exactly the `<`/`!=` `write_field_cmp` writes down.
                let (ne, lt) = if ty.is_float() {
                    (
                        self.builder.build_fcmp(RealCmp::One, av, bv, "ne"),
                        self.builder.build_fcmp(RealCmp::Olt, av, bv, "lt"),
                    )
                } else {
                    let pred = if is_signed(ty) { IntCmp::Slt } else { IntCmp::Ult };
                    (
                        self.builder.build_icmp(IntCmp::Ne, av, bv, "ne"),
                        self.builder.build_icmp(pred, av, bv, "lt"),
                    )
                };
                let diff = func.append_block("cmp.diff");
                let cont = func.append_block("cmp.cont");
                self.builder.build_cond_br(ne, diff, cont);
                self.builder.position_at_end(diff);
                let sign = self.builder.build_select(lt, self.v_i32(u64::MAX), self.v_i32(1), "sign");
                self.builder.build_ret(sign);
                self.builder.position_at_end(cont);
                Ok(cont)
            }
        }
    }

    fn emit_enum_clone(&self, e: &crate::types::EnumDef) -> Result<(), String> {
        let Some((func, _)) = self.clone_fns.get(&e.name).copied() else {
            return Ok(());
        };
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let src = func.get_param(0);
        let dst = func.get_param(1);
        let tag_ptr = self.fld(info.llvm_ty, src, ENUM_TAG_FIELD, "tag");
        let tag = self.ld(self.ctx.i32(), tag_ptr, "tag");
        self.st(tag, self.fld(info.llvm_ty, dst, ENUM_TAG_FIELD, "tag"));
        let mut cont = entry;
        for (i, v) in e.variants.iter().enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            self.builder.position_at_end(cont);
            let is_i = self
                .builder
                .build_icmp(IntCmp::Eq, tag, self.v_i32(i as u64), "isvar");
            let then_bb = func.append_block("clone.v");
            let next = func.append_block("clone.next");
            self.builder.build_cond_br(is_i, then_bb, next);
            self.builder.position_at_end(then_bb);
            let src_union = self.fld(info.llvm_ty, src, ENUM_UNION_FIELD, "kelp_v");
            let dst_union = self.fld(info.llvm_ty, dst, ENUM_UNION_FIELD, "kelp_v");
            for (fi, field) in v.fields.iter().enumerate() {
                let s = self.fld(
                    info.variants[i].payload_ty,
                    src_union,
                    fi as u32,
                    "f",
                );
                let d = self.fld(
                    info.variants[i].payload_ty,
                    dst_union,
                    fi as u32,
                    "f",
                );
                self.clone_into(s, d, &field.ty)?;
            }
            self.builder.build_br(next);
            cont = next;
        }
        self.builder.position_at_end(cont);
        self.builder.build_ret_void();
        Ok(())
    }

    fn emit_enum_cmp(&self, e: &crate::types::EnumDef) -> Result<(), String> {
        let Some((func, _)) = self.cmp_fns.get(&e.name).copied() else {
            return Ok(());
        };
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let a = func.get_param(0);
        let b = func.get_param(1);
        let ta = self.ld(
            self.ctx.i32(),
            self.fld(info.llvm_ty, a, ENUM_TAG_FIELD, "tag"),
            "ta",
        );
        let tb = self.ld(
            self.ctx.i32(),
            self.fld(info.llvm_ty, b, ENUM_TAG_FIELD, "tag"),
            "tb",
        );
        // A different tag orders first, so every value of one variant sorts
        // together.
        let ne = self.builder.build_icmp(IntCmp::Ne, ta, tb, "tagdiff");
        let diff = func.append_block("cmp.tag");
        let cont0 = func.append_block("cmp.variants");
        self.builder.build_cond_br(ne, diff, cont0);
        self.builder.position_at_end(diff);
        let lt = self.builder.build_icmp(IntCmp::Slt, ta, tb, "lt");
        let sign = self.builder.build_select(lt, self.v_i32(u64::MAX), self.v_i32(1), "sign");
        self.builder.build_ret(sign);

        let mut cont = cont0;
        for (i, v) in e.variants.iter().enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            self.builder.position_at_end(cont);
            let is_i = self
                .builder
                .build_icmp(IntCmp::Eq, ta, self.v_i32(i as u64), "isvar");
            let then_bb = func.append_block("cmp.v");
            let next = func.append_block("cmp.next");
            self.builder.build_cond_br(is_i, then_bb, next);
            self.builder.position_at_end(then_bb);
            let a_union = self.fld(info.llvm_ty, a, ENUM_UNION_FIELD, "kelp_v");
            let b_union = self.fld(info.llvm_ty, b, ENUM_UNION_FIELD, "kelp_v");
            let mut inner = then_bb;
            for (fi, field) in v.fields.iter().enumerate() {
                self.builder.position_at_end(inner);
                let ap = self.fld(info.variants[i].payload_ty, a_union, fi as u32, "f");
                let bp = self.fld(info.variants[i].payload_ty, b_union, fi as u32, "f");
                inner = self.emit_field_cmp(func, ap, bp, &field.ty)?;
            }
            self.builder.position_at_end(inner);
            self.builder.build_br(next);
            cont = next;
        }
        self.builder.position_at_end(cont);
        self.builder.build_ret(self.v_i32(0));
        Ok(())
    }

    /// One `List<T>` function set, the mirror of `codegen_c::write_list` and
    /// `write_list_s1`. The list is `{ data, len, cap }`, the element's size and
    /// drop decide the bodies, and every bounds check panics with the C
    /// backend's own text.
    fn emit_list_bodies(&self, elem: &Ty) -> Result<(), String> {
        let base = list_c_type(elem);
        let list_ty = self.cty(&Ty::List(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let elem_size = self.layout.abi_size_of(elem_ty);
        let ptr = self.pty();
        let (i1t, i32t, i64t) = (self.ctx.i1(), self.ctx.i32(), self.ctx.i64());
        let drop_elem = needs_drop(elem, &self.owned);
        let sortable = crate::types::value_is_sortable(elem, &self.sortable);
        let clonable = crate::types::value_is_clonable(elem, &self.clonable);
        let comparable = crate::types::list_comparable_elem(elem);

        // --- empty ---
        {
            let _ = self.start(&format!("{base}__empty"), "entry")?;
            let l = self.builder.build_alloca(list_ty, "l");
            self.st(self.ctx.const_null(list_ty), l);
            let v = self.ld(list_ty, l, "ret");
            self.builder.build_ret(v);
        }

        // --- reserve(self, want) ---
        {
            let (f, _) = self.start(&format!("{base}__reserve"), "entry")?;
            let self_p = f.get_param(0);
            let want = f.get_param(1);
            let cap = self.ld(i64t, self.fld(list_ty, self_p, L_CAP, "cap"), "cap");
            let enough = self.builder.build_icmp(IntCmp::Ule, want, cap, "enough");
            let done = self.block(f, "ret");
            let grow = self.block(f, "grow");
            self.builder.build_cond_br(enough, done, grow);
            self.builder.position_at_end(grow);
            let cap_slot = self.builder.build_alloca(i64t, "cap");
            let zero = self.builder.build_icmp(IntCmp::Eq, cap, self.v_i64(0), "zero");
            let start = self.builder.build_select(zero, self.v_i64(4), cap, "start");
            self.st(start, cap_slot);
            let header = self.loop_header(f, "grow.cond");
            let c = self.ld(i64t, cap_slot, "cap");
            let need = self.builder.build_icmp(IntCmp::Ult, c, want, "need");
            let body = self.block(f, "grow.body");
            let exit = self.block(f, "grow.end");
            self.builder.build_cond_br(need, body, exit);
            self.builder.position_at_end(body);
            let c2 = self.ld(i64t, cap_slot, "cap");
            let doubled = self.builder.build_int_binop(IntBinOp::Mul, c2, self.v_i64(2), "cap2");
            self.st(doubled, cap_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let final_cap = self.ld(i64t, cap_slot, "cap");
            let size = self
                .builder
                .build_int_binop(IntBinOp::Mul, final_cap, self.v_i64(elem_size), "bytes");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let (realloc, rty) = self.libc_fn("realloc")?;
            let buf = self.call(realloc, rty, &[data, size], "buf");
            self.check_alloc(buf, f)?;
            self.st(buf, self.fld(list_ty, self_p, L_DATA, "data"));
            self.st(final_cap, self.fld(list_ty, self_p, L_CAP, "cap"));
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }

        // --- of(v) ---
        {
            let (f, _) = self.start(&format!("{base}__of"), "entry")?;
            let v = f.get_param(0);
            let l = self.builder.build_alloca(list_ty, "l");
            self.st(self.ctx.const_null(list_ty), l);
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[l, self.v_i64(1)], "");
            let data = self.ld(ptr, self.fld(list_ty, l, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, self.v_i64(0), "at");
            self.st(v, addr);
            self.st(self.v_i64(1), self.fld(list_ty, l, L_LEN, "len"));
            let out = self.ld(list_ty, l, "of");
            self.builder.build_ret(out);
        }

        // --- len / is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__len"), "entry")?;
            let n = self.ld(i64t, self.fld(list_ty, f.get_param(0), L_LEN, "len"), "len");
            self.builder.build_ret(n);
        }
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let n = self.ld(i64t, self.fld(list_ty, f.get_param(0), L_LEN, "len"), "len");
            let e = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
            self.builder.build_ret(e);
        }

        // --- push(self, v) ---
        {
            let (f, _) = self.start(&format!("{base}__push"), "entry")?;
            let self_p = f.get_param(0);
            let v = f.get_param(1);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let want = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "want");
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[self_p, want], "");
            let n = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, n, "at");
            self.st(v, addr);
            let next = self.builder.build_int_binop(IntBinOp::Add, n, self.v_i64(1), "len1");
            self.st(next, self.fld(list_ty, self_p, L_LEN, "len"));
            self.builder.build_ret_void();
        }

        // --- bounds(self, i) ---
        {
            let (f, _) = self.start(&format!("{base}__bounds"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let bad = self.builder.build_icmp(IntCmp::Uge, i, len, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "inbounds");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.get: index out of bounds")?;
            self.builder.position_at_end(ok);
            self.builder.build_ret_void();
        }

        // --- get / get_const ---
        if elem.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[self_p, i], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, i, "at");
            let v = self.ld(elem_ty, addr, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[self_p, i], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, i, "at");
            self.builder.build_ret(addr);
        }

        // --- set(self, i, v) ---
        {
            let (f, _) = self.start(&format!("{base}__set"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let v = f.get_param(2);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[self_p, i], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, i, "at");
            if drop_elem {
                self.call_drop(addr, elem)?;
            }
            self.st(v, addr);
            self.builder.build_ret_void();
        }

        // --- pop(self) ---
        {
            let (f, _) = self.start(&format!("{base}__pop"), "entry")?;
            let self_p = f.get_param(0);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let empty = self.builder.build_icmp(IntCmp::Eq, len, self.v_i64(0), "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "pop");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("List.pop: the list is empty")?;
            self.builder.position_at_end(ok);
            let last = self.builder.build_int_binop(IntBinOp::Sub, len, self.v_i64(1), "last");
            self.st(last, self.fld(list_ty, self_p, L_LEN, "len"));
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, last, "at");
            let v = self.ld(elem_ty, addr, "pop");
            self.builder.build_ret(v);
        }

        // --- clear(self) ---
        {
            let (f, _) = self.start(&format!("{base}__clear"), "entry")?;
            let self_p = f.get_param(0);
            if drop_elem {
                let i_slot = self.builder.build_alloca(i64t, "i");
                self.st(self.v_i64(0), i_slot);
                let header = self.loop_header(f, "clear.cond");
                let i = self.ld(i64t, i_slot, "i");
                let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
                let more = self.builder.build_icmp(IntCmp::Ult, i, len, "more");
                let body = self.block(f, "clear.body");
                let exit = self.block(f, "clear.end");
                self.builder.build_cond_br(more, body, exit);
                self.builder.position_at_end(body);
                let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
                let addr = self.gep(elem_ty, data, i, "at");
                self.call_drop(addr, elem)?;
                let next = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
                self.st(next, i_slot);
                self.builder.build_br(header);
                self.builder.position_at_end(exit);
            }
            self.st(self.v_i64(0), self.fld(list_ty, self_p, L_LEN, "len"));
            self.builder.build_ret_void();
        }

        // --- reverse(self) ---
        {
            let (f, _) = self.start(&format!("{base}__reverse"), "entry")?;
            let self_p = f.get_param(0);
            let i_slot = self.builder.build_alloca(i64t, "i");
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(self.v_i64(0), i_slot);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            self.st(len, j_slot);
            let header = self.loop_header(f, "rev.cond");
            let i = self.ld(i64t, i_slot, "i");
            let j = self.ld(i64t, j_slot, "j");
            let i1 = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            let more = self.builder.build_icmp(IntCmp::Ult, i1, j, "more");
            let body = self.block(f, "rev.body");
            let exit = self.block(f, "rev.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let jm = self.builder.build_int_binop(IntBinOp::Sub, j, self.v_i64(1), "j1");
            self.st(jm, j_slot);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let ai = self.gep(elem_ty, data, i, "ai");
            let aj = self.gep(elem_ty, data, jm, "aj");
            let tv = self.ld(elem_ty, ai, "t");
            let u = self.ld(elem_ty, aj, "u");
            self.st(u, ai);
            self.st(tv, aj);
            let inx = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(inx, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            self.builder.build_ret_void();
        }

        // --- insert(self, i, v) ---
        {
            let (f, _) = self.start(&format!("{base}__insert"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let v = f.get_param(2);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let bad = self.builder.build_icmp(IntCmp::Ugt, i, len, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "insert");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.insert: index out of bounds")?;
            self.builder.position_at_end(ok);
            let want = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "want");
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[self_p, want], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(len, j_slot);
            let header = self.loop_header(f, "shift.cond");
            let j = self.ld(i64t, j_slot, "j");
            let more = self.builder.build_icmp(IntCmp::Ugt, j, i, "more");
            let body = self.block(f, "shift.body");
            let exit = self.block(f, "shift.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let jm = self.builder.build_int_binop(IntBinOp::Sub, j, self.v_i64(1), "j1");
            let src = self.gep(elem_ty, data, jm, "src");
            let dst = self.gep(elem_ty, data, j, "dst");
            let sv = self.ld(elem_ty, src, "sv");
            self.st(sv, dst);
            self.st(jm, j_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let at = self.gep(elem_ty, data, i, "at");
            self.st(v, at);
            let len1 = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "len1");
            self.st(len1, self.fld(list_ty, self_p, L_LEN, "len"));
            self.builder.build_ret_void();
        }

        // --- remove(self, i) ---
        {
            let (f, _) = self.start(&format!("{base}__remove"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let bad = self.builder.build_icmp(IntCmp::Uge, i, len, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "remove");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.remove: index out of bounds")?;
            self.builder.position_at_end(ok);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let out_slot = self.builder.build_alloca(elem_ty, "out");
            let at = self.gep(elem_ty, data, i, "at");
            self.st(self.ld(elem_ty, at, "v"), out_slot);
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(i, j_slot);
            let header = self.loop_header(f, "shift.cond");
            let j = self.ld(i64t, j_slot, "j");
            let j1 = self.builder.build_int_binop(IntBinOp::Add, j, self.v_i64(1), "j1");
            let more = self.builder.build_icmp(IntCmp::Ult, j1, len, "more");
            let body = self.block(f, "shift.body");
            let exit = self.block(f, "shift.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let src = self.gep(elem_ty, data, j1, "src");
            let dst = self.gep(elem_ty, data, j, "dst");
            let sv = self.ld(elem_ty, src, "sv");
            self.st(sv, dst);
            self.st(j1, j_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let len1 = self.builder.build_int_binop(IntBinOp::Sub, len, self.v_i64(1), "len1");
            self.st(len1, self.fld(list_ty, self_p, L_LEN, "len"));
            let out = self.ld(elem_ty, out_slot, "removed");
            self.builder.build_ret(out);
        }

        // --- cmp(a, b) and sort(self) ---
        if sortable {
            let (f, _) = self.start(&format!("{base}__cmp"), "entry")?;
            let a = f.get_param(0);
            let b = f.get_param(1);
            let av = self.ld(elem_ty, a, "a");
            let bv = self.ld(elem_ty, b, "b");
            let out = match elem {
                Ty::Bool => {
                    let ax = self.builder.build_int_cast(false, true, av, i32t, "ax");
                    let bx = self.builder.build_int_cast(false, true, bv, i32t, "bx");
                    self.builder.build_int_binop(IntBinOp::Sub, ax, bx, "cmp")
                }
                Ty::String | Ty::Bytes => {
                    let alen = self.ld(i64t, self.fld(elem_ty, a, 1, "alen"), "alen");
                    let blen = self.ld(i64t, self.fld(elem_ty, b, 1, "blen"), "blen");
                    let min = self.builder.build_select(
                        self.builder.build_icmp(IntCmp::Ult, alen, blen, "lt"),
                        alen,
                        blen,
                        "n",
                    );
                    let adata = self.ld(ptr, self.fld(elem_ty, a, 0, "ad"), "ad");
                    let bdata = self.ld(ptr, self.fld(elem_ty, b, 0, "bd"), "bd");
                    let (memcmp, mty) = self.libc_fn("memcmp")?;
                    let c = self.call(memcmp, mty, &[adata, bdata, min], "memcmp");
                    let c1 = self.builder.build_select(
                        self.builder.build_icmp(IntCmp::Slt, c, self.v_i32(0), "neg"),
                        self.v_i32(u64::MAX),
                        self.v_i32(1),
                        "c1",
                    );
                    let npos = self.builder.build_icmp(IntCmp::Ugt, min, self.v_i64(0), "npos");
                    let cnonzero = self.builder.build_icmp(IntCmp::Ne, c, self.v_i32(0), "cnz");
                    let taken = self
                        .builder
                        .build_int_binop(IntBinOp::And, npos, cnonzero, "taken");
                    let llt = self.builder.build_icmp(IntCmp::Ult, alen, blen, "llt");
                    let lgt = self.builder.build_icmp(IntCmp::Ugt, alen, blen, "lgt");
                    let len_cmp = self.builder.build_select(
                        llt,
                        self.v_i32(u64::MAX),
                        self.builder.build_select(lgt, self.v_i32(1), self.v_i32(0), "l0"),
                        "lc",
                    );
                    self.builder.build_select(taken, c1, len_cmp, "cmp")
                }
                Ty::Struct(name) => {
                    let (cmp, cty2) = self
                        .cmp_fns
                        .get(name)
                        .copied()
                        .ok_or_else(|| no_value(&format!("the order of '{name}'")))?;
                    self.call(cmp, cty2, &[a, b], "cmp")
                }
                _ => {
                    // `(*a > *b) - (*a < *b)`, with `fcmp` for a float element
                    // (a `List<f64>` sorts, and C's `>`/`<` on a double are
                    // `fcmp ogt`/`olt`).
                    let (gt, lt) = if elem.is_float() {
                        (
                            self.builder.build_fcmp(RealCmp::Ogt, av, bv, "gt"),
                            self.builder.build_fcmp(RealCmp::Olt, av, bv, "lt"),
                        )
                    } else {
                        let pred_gt = if is_signed(elem) { IntCmp::Sgt } else { IntCmp::Ugt };
                        let pred_lt = if is_signed(elem) { IntCmp::Slt } else { IntCmp::Ult };
                        (
                            self.builder.build_icmp(pred_gt, av, bv, "gt"),
                            self.builder.build_icmp(pred_lt, av, bv, "lt"),
                        )
                    };
                    let g = self.builder.build_int_cast(false, true, gt, i32t, "g");
                    let l = self.builder.build_int_cast(false, true, lt, i32t, "l");
                    self.builder.build_int_binop(IntBinOp::Sub, g, l, "cmp")
                }
            };
            self.builder.build_ret(out);

            // sort(self): insertion sort, stable and in place, the mirror of
            // `write_list_s1`'s loop.
            let (f, _) = self.start(&format!("{base}__sort"), "entry")?;
            let self_p = f.get_param(0);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let i_slot = self.builder.build_alloca(i64t, "i");
            let j_slot = self.builder.build_alloca(i64t, "j");
            let v_slot = self.builder.build_alloca(elem_ty, "v");
            self.st(self.v_i64(1), i_slot);
            let outer = self.loop_header(f, "sort.cond");
            let i = self.ld(i64t, i_slot, "i");
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let more = self.builder.build_icmp(IntCmp::Ult, i, len, "more");
            let obody = self.block(f, "sort.body");
            let oexit = self.block(f, "sort.end");
            self.builder.build_cond_br(more, obody, oexit);
            self.builder.position_at_end(obody);
            let at = self.gep(elem_ty, data, i, "at");
            self.st(self.ld(elem_ty, at, "v"), v_slot);
            self.st(i, j_slot);
            let inner = self.loop_header(f, "sort.inner");
            let j = self.ld(i64t, j_slot, "j");
            let jpos = self.builder.build_icmp(IntCmp::Ugt, j, self.v_i64(0), "jpos");
            let cond = self.block(f, "sort.inner.cond");
            let iexit = self.block(f, "sort.inner.end");
            self.builder.build_cond_br(jpos, cond, iexit);
            self.builder.position_at_end(cond);
            let jm = self.builder.build_int_binop(IntBinOp::Sub, j, self.v_i64(1), "j1");
            let prev = self.gep(elem_ty, data, jm, "prev");
            let (cmpsym, cmpty) = self.cfn(&format!("{base}__cmp"))?;
            let c = self.call(cmpsym, cmpty, &[prev, v_slot], "cmp");
            let swap = self.builder.build_icmp(IntCmp::Sgt, c, self.v_i32(0), "gt");
            let ibody = self.block(f, "sort.inner.body");
            self.builder.build_cond_br(swap, ibody, iexit);
            self.builder.position_at_end(ibody);
            let dst = self.gep(elem_ty, data, j, "dst");
            self.st(self.ld(elem_ty, prev, "pv"), dst);
            self.st(jm, j_slot);
            self.builder.build_br(inner);
            self.builder.position_at_end(iexit);
            let jfin = self.ld(i64t, j_slot, "j");
            let slot = self.gep(elem_ty, data, jfin, "slot");
            self.st(self.ld(elem_ty, v_slot, "vv"), slot);
            let inext = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(inext, i_slot);
            self.builder.build_br(outer);
            self.builder.position_at_end(oexit);
            self.builder.build_ret_void();
        }

        // --- contains(self, v) ---
        if comparable {
            let (f, _) = self.start(&format!("{base}__contains"), "entry")?;
            let self_p = f.get_param(0);
            let v = f.get_param(1);
            let out_slot = self.builder.build_alloca(i1t, "found");
            let i_slot = self.builder.build_alloca(i64t, "i");
            self.st(self.ctx.const_bool(false), out_slot);
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "has.cond");
            let i = self.ld(i64t, i_slot, "i");
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let more = self.builder.build_icmp(IntCmp::Ult, i, len, "more");
            let body = self.block(f, "has.body");
            let exit = self.block(f, "has.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let at = self.gep(elem_ty, data, i, "at");
            let hit = match elem {
                Ty::String | Ty::Bytes => {
                    let alen = self.ld(i64t, self.fld(elem_ty, at, 1, "alen"), "alen");
                    let vlen = self.ld(i64t, self.fld(elem_ty, v, 1, "vlen"), "vlen");
                    let leq = self.builder.build_icmp(IntCmp::Eq, alen, vlen, "leq");
                    let zero = self.builder.build_icmp(IntCmp::Eq, vlen, self.v_i64(0), "z");
                    // `zleq` is computed in `body`, so the phi is the first
                    // instruction of `after` (a phi must be grouped at the top).
                    let zeros = self.builder.build_int_binop(IntBinOp::And, zero, leq, "zleq");
                    let data_cmp = self.block(f, "has.cmp");
                    let after = self.block(f, "has.after");
                    self.builder.build_cond_br(zero, after, data_cmp);
                    self.builder.position_at_end(data_cmp);
                    let adata = self.ld(ptr, self.fld(elem_ty, at, 0, "ad"), "ad");
                    let vdata = self.ld(ptr, self.fld(elem_ty, v, 0, "vd"), "vd");
                    let (memcmp, mty) = self.libc_fn("memcmp")?;
                    let c = self.call(memcmp, mty, &[adata, vdata, vlen], "memcmp");
                    let c0 = self.builder.build_icmp(IntCmp::Eq, c, self.v_i32(0), "c0");
                    // Length first, then the bytes: `"app"` must not equal
                    // `"apple"` even though `memcmp` over the shorter prefix
                    // says the bytes agree.
                    let ceq = self.builder.build_int_binop(IntBinOp::And, leq, c0, "ceq");
                    self.builder.build_br(after);
                    self.builder.position_at_end(after);
                    let hit_phi = self.builder.build_phi(i1t, "hit");
                    self.builder.add_incoming(hit_phi, &[(zeros, body), (ceq, data_cmp)]);
                    hit_phi
                }
                // A float element is `Copy`, so `contains` is offered for it
                // and C's `==` on a double is `fcmp oeq`; an integer compare
                // here is invalid IR on the emitted `double`.
                _ if elem.is_float() => {
                    self.builder
                        .build_fcmp(RealCmp::Oeq, self.ld(elem_ty, at, "av"), v, "hit")
                }
                _ => self.builder.build_icmp(IntCmp::Eq, self.ld(elem_ty, at, "av"), v, "hit"),
            };
            let found = self.block(f, "has.found");
            let next = self.block(f, "has.next");
            self.builder.build_cond_br(hit, found, next);
            self.builder.position_at_end(found);
            self.st(self.ctx.const_bool(true), out_slot);
            self.builder.build_br(exit);
            self.builder.position_at_end(next);
            let inext = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(inext, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(i1t, out_slot, "found");
            self.builder.build_ret(out);
        }

        // --- slice(self, from, to) and clone(self) ---
        if clonable {
            let (f, _) = self.start(&format!("{base}__slice"), "entry")?;
            let self_p = f.get_param(0);
            let from = f.get_param(1);
            let to = f.get_param(2);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let a = self.builder.build_icmp(IntCmp::Ugt, from, to, "from_gt_to");
            let b = self.builder.build_icmp(IntCmp::Ugt, to, len, "to_gt_len");
            let bad = self.builder.build_int_binop(IntBinOp::Or, a, b, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "slice");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.slice: range out of bounds")?;
            self.builder.position_at_end(ok);
            let out_slot = self.builder.build_alloca(list_ty, "out");
            self.st(self.ctx.const_null(list_ty), out_slot);
            let n = self.builder.build_int_binop(IntBinOp::Sub, to, from, "n");
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[out_slot, n], "");
            let i_slot = self.builder.build_alloca(i64t, "i");
            self.st(from, i_slot);
            let header = self.loop_header(f, "slice.cond");
            let i = self.ld(i64t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, to, "more");
            let body = self.block(f, "slice.body");
            let exit = self.block(f, "slice.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let src = self.gep(elem_ty, data, i, "src");
            let olen = self.ld(i64t, self.fld(list_ty, out_slot, L_LEN, "olen"), "olen");
            let odata = self.ld(ptr, self.fld(list_ty, out_slot, L_DATA, "odata"), "odata");
            let dst = self.gep(elem_ty, odata, olen, "dst");
            self.clone_into(src, dst, elem)?;
            let olen1 = self.builder.build_int_binop(IntBinOp::Add, olen, self.v_i64(1), "olen1");
            self.st(olen1, self.fld(list_ty, out_slot, L_LEN, "len"));
            let inext = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(inext, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(list_ty, out_slot, "slice");
            self.builder.build_ret(out);

            let (f, _) = self.start(&format!("{base}__clone"), "entry")?;
            let self_p = f.get_param(0);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let (slice, sty) = self.cfn(&format!("{base}__slice"))?;
            let out = self.call(slice, sty, &[self_p, self.v_i64(0), len], "clone");
            self.builder.build_ret(out);
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(&drop_fn_name(&Ty::List(Box::new(elem.clone()))), "entry")?;
            let self_p = f.get_param(0);
            let (clear, cty2) = self.cfn(&format!("{base}__clear"))?;
            self.call(clear, cty2, &[self_p], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let (free, fty) = self.libc_fn("free")?;
            self.call(free, fty, &[data], "");
            self.st(self.ctx.const_null(ptr), self.fld(list_ty, self_p, L_DATA, "data"));
            self.st(self.v_i64(0), self.fld(list_ty, self_p, L_CAP, "cap"));
            self.builder.build_ret_void();
        }
        Ok(())
    }

    // ---- container bodies -------------------------------------------------

    fn emit_container_bodies(&self) -> Result<(), String> {
        for elem in self.lists_sorted() {
            self.emit_list_bodies(&elem)?;
        }
        for elem in self.boxes_sorted() {
            self.emit_box_bodies(&elem)?;
        }
        for (key, value) in self.maps_sorted() {
            self.emit_map_bodies(&key, &value)?;
        }
        for arr in self.arrays_sorted() {
            if let Ty::Array(elem, len) = arr {
                self.emit_array_bodies(&elem, len)?;
            }
        }
        Ok(())
    }

    /// One `Box<T>` function set, the mirror of `codegen_c::write_box`. The box
    /// holds one pointer or `NULL`; `take` moves the value out, which is what
    /// makes a recursive `datac` expressible without `Option`.
    fn emit_box_bodies(&self, elem: &Ty) -> Result<(), String> {
        let base = box_c_type(elem);
        let box_ty = self.cty(&Ty::Box(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let ptr = self.pty();
        let i1t = self.ctx.i1();
        let drop_elem = needs_drop(elem, &self.owned);
        let null = self.ctx.const_null(ptr);

        // --- empty ---
        {
            let _ = self.start(&format!("{base}__empty"), "entry")?;
            let b = self.builder.build_alloca(box_ty, "b");
            self.st(null, self.fld(box_ty, b, 0, "value"));
            let v = self.ld(box_ty, b, "empty");
            self.builder.build_ret(v);
        }

        // --- of(v) ---
        {
            let (f, _) = self.start(&format!("{base}__of"), "entry")?;
            let v = f.get_param(0);
            let b = self.builder.build_alloca(box_ty, "b");
            let p = self.malloc_for(elem_ty)?;
            self.check_alloc(p, f)?;
            self.st(v, p);
            self.st(p, self.fld(box_ty, b, 0, "value"));
            let out = self.ld(box_ty, b, "of");
            self.builder.build_ret(out);
        }

        // --- is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let value = self.ld(ptr, self.fld(box_ty, f.get_param(0), 0, "value"), "value");
            let e = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            self.builder.build_ret(e);
        }

        // --- get / get_const ---
        if elem.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let value = self.ld(ptr, self.fld(box_ty, f.get_param(0), 0, "value"), "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Box.get: the box is empty")?;
            self.builder.position_at_end(ok);
            let v = self.ld(elem_ty, value, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let value = self.ld(ptr, self.fld(box_ty, f.get_param(0), 0, "value"), "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Box.get: the box is empty")?;
            self.builder.position_at_end(ok);
            self.builder.build_ret(value);
        }

        // --- set(self, v) ---
        {
            let (f, _) = self.start(&format!("{base}__set"), "entry")?;
            let self_p = f.get_param(0);
            let v = f.get_param(1);
            let slot = self.fld(box_ty, self_p, 0, "value");
            let value = self.ld(ptr, slot, "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let null_bb = self.block(f, "alloc");
            let have_bb = self.block(f, "have");
            let stored = self.block(f, "store");
            self.builder.build_cond_br(empty, null_bb, have_bb);
            self.builder.position_at_end(null_bb);
            let p = self.malloc_for(elem_ty)?;
            self.check_alloc(p, f)?;
            self.st(p, slot);
            self.builder.build_br(stored);
            self.builder.position_at_end(have_bb);
            if drop_elem {
                self.call_drop(value, elem)?;
            }
            self.builder.build_br(stored);
            self.builder.position_at_end(stored);
            let now = self.ld(ptr, slot, "value");
            self.st(v, now);
            self.builder.build_ret_void();
        }

        // --- take(self) ---
        {
            let (f, _) = self.start(&format!("{base}__take"), "entry")?;
            let self_p = f.get_param(0);
            let slot = self.fld(box_ty, self_p, 0, "value");
            let value = self.ld(ptr, slot, "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "take");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Box.take: the box is empty")?;
            self.builder.position_at_end(ok);
            let v = self.ld(elem_ty, value, "take");
            let (free, fty) = self.libc_fn("free")?;
            self.call(free, fty, &[value], "");
            self.st(null, slot);
            self.builder.build_ret(v);
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(&drop_fn_name(&Ty::Box(Box::new(elem.clone()))), "entry")?;
            let self_p = f.get_param(0);
            let slot = self.fld(box_ty, self_p, 0, "value");
            let value = self.ld(ptr, slot, "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let done = self.block(f, "ret");
            let live = self.block(f, "live");
            self.builder.build_cond_br(empty, done, live);
            self.builder.position_at_end(live);
            if drop_elem {
                self.call_drop(value, elem)?;
            }
            let (free, fty) = self.libc_fn("free")?;
            self.call(free, fty, &[value], "");
            self.st(null, slot);
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }
        let _ = i1t;
        Ok(())
    }

    /// One `[T; N]` function set, the mirror of `codegen_c::write_array`. The
    /// array is `{ T v[N] }` held inline, so nothing here allocates; `len()` is
    /// the type's own constant and `get`/`set` are the same bounds check
    /// `List` uses.
    fn emit_array_bodies(&self, elem: &Ty, len: u64) -> Result<(), String> {
        let base = crate::types::array_c_type(elem, len);
        let array_ty = self.cty(&Ty::Array(Box::new(elem.clone()), len))?;
        let elem_ty = self.lty2(elem)?;
        let i64t = self.ctx.i64();
        let i1t = self.ctx.i1();
        let drop_elem = needs_drop(elem, &self.owned);
        let n = self.v_i64(len);

        // --- len / is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__len"), "entry")?;
            let _ = f;
            self.builder.build_ret(n);
        }
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let _ = f;
            self.builder.build_ret(self.ctx.const_bool(false));
        }

        // --- bounds(i) ---
        {
            let (f, _) = self.start(&format!("{base}__bounds"), "entry")?;
            let i = f.get_param(0);
            let bad = self.builder.build_icmp(IntCmp::Uge, i, n, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "inbounds");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("Array.get: index out of bounds")?;
            self.builder.position_at_end(ok);
            self.builder.build_ret_void();
        }

        // --- get / get_const ---
        if elem.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[i], "");
            let v0 = self.fld(array_ty, self_p, 0, "v");
            let addr = self.gep(elem_ty, v0, i, "at");
            let v = self.ld(elem_ty, addr, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[i], "");
            let v0 = self.fld(array_ty, self_p, 0, "v");
            let addr = self.gep(elem_ty, v0, i, "at");
            self.builder.build_ret(addr);
        }

        // --- set(self, i, v) ---
        {
            let (f, _) = self.start(&format!("{base}__set"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let v = f.get_param(2);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[i], "");
            let v0 = self.fld(array_ty, self_p, 0, "v");
            let addr = self.gep(elem_ty, v0, i, "at");
            if drop_elem {
                self.call_drop(addr, elem)?;
            }
            self.st(v, addr);
            self.builder.build_ret_void();
        }

        // --- fill(v) ---
        {
            let (f, _) = self.start(&format!("{base}__fill"), "entry")?;
            let v = f.get_param(0);
            let out = self.builder.build_alloca(array_ty, "r");
            let v0 = self.fld(array_ty, out, 0, "v");
            let i_slot = self.builder.build_alloca(i64t, "i");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "fill.cond");
            let i = self.ld(i64t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
            let body = self.block(f, "fill.body");
            let exit = self.block(f, "fill.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let addr = self.gep(elem_ty, v0, i, "at");
            self.st(v, addr);
            let next = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(next, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out_v = self.ld(array_ty, out, "fill");
            self.builder.build_ret(out_v);
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(
                &drop_fn_name(&Ty::Array(Box::new(elem.clone()), len)),
                "entry",
            )?;
            let self_p = f.get_param(0);
            if drop_elem {
                let v0 = self.fld(array_ty, self_p, 0, "v");
                let i_slot = self.builder.build_alloca(i64t, "i");
                self.st(self.v_i64(0), i_slot);
                let header = self.loop_header(f, "drop.cond");
                let i = self.ld(i64t, i_slot, "i");
                let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
                let body = self.block(f, "drop.body");
                let exit = self.block(f, "drop.end");
                self.builder.build_cond_br(more, body, exit);
                self.builder.position_at_end(body);
                let addr = self.gep(elem_ty, v0, i, "at");
                self.call_drop(addr, elem)?;
                let next = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
                self.st(next, i_slot);
                self.builder.build_br(header);
                self.builder.position_at_end(exit);
            }
            self.builder.build_ret_void();
        }
        let _ = i1t;
        Ok(())
    }

    fn map_key_arg(&self, key: &Ty) -> Result<Type<'a>, String> {
        if key == &Ty::String {
            Ok(self.pty())
        } else {
            self.lty2(key)
        }
    }

    fn map_specs(&self, key: &Ty, value: &Ty) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = map_c_type(key, value);
        let map_ty = self.cty(&Ty::Map(Box::new(key.clone()), Box::new(value.clone())))?;
        let value_ty = self.lty2(value)?;
        let key_arg = self.map_key_arg(key)?;
        let list_key_ty = self.cty(&Ty::List(Box::new(key.clone())))?;
        let list_value_ty = self.cty(&Ty::List(Box::new(value.clone())))?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let (i1t, i64t) = (self.ctx.i1(), self.ctx.i64());
        let clonable = list_clonable_elem(value);
        let mut out = vec![
            spec(&format!("{base}__new"), map_ty, &[]),
            spec(&format!("{base}__find"), i64t, &[ptr, key_arg, ptr]),
            spec(&format!("{base}__rehash"), void, &[ptr, i64t]),
            spec(&format!("{base}__insert"), void, &[ptr, key_arg, value_ty]),
            spec(&format!("{base}__has"), i1t, &[ptr, key_arg]),
        ];
        if value.is_copy() {
            out.push(spec(&format!("{base}__get"), value_ty, &[ptr, key_arg]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr, key_arg]));
        }
        if clonable {
            out.push(spec(
                &format!("{base}__get_or"),
                value_ty,
                &[ptr, key_arg, value_ty],
            ));
        }
        out.push(spec(&format!("{base}__remove"), value_ty, &[ptr, key_arg]));
        out.push(spec(&format!("{base}__clear"), void, &[ptr]));
        out.push(spec(&format!("{base}__len"), i64t, &[ptr]));
        out.push(spec(&format!("{base}__is_empty"), i1t, &[ptr]));
        out.push(spec(&format!("{base}__keys"), list_key_ty, &[ptr]));
        if clonable {
            out.push(spec(&format!("{base}__values"), list_value_ty, &[ptr]));
        }
        out.push(spec(
            &drop_fn_name(&Ty::Map(Box::new(key.clone()), Box::new(value.clone()))),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    /// `kelp_map_hash`/`kelp_map_mix` over a key *argument* (a `str` view for a
    /// `String` key, the value itself for a scalar), the mirror of
    /// `write_map`'s `hash_arg`.
    fn map_key_hash_arg(&self, key: &Ty, k: Value<'a>) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let (strlen, sty) = self.libc_fn("strlen")?;
            let n = self.call(strlen, sty, &[k], "len");
            let rt = self.rt("kelp_map_hash")?;
            Ok(self.call(rt.func, rt.ty, &[k, n], "hash"))
        } else {
            let wide = self.key_to_u64(key, k)?;
            let rt = self.rt("kelp_map_mix")?;
            Ok(self.call(rt.func, rt.ty, &[wide], "hash"))
        }
    }

    /// The same hash over a *stored* key (a `kelp_String` inline in the entry,
    /// or a scalar).
    fn map_key_hash_stored(
        &self,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let data = self.ld(self.pty(), self.fld(self.lty2(key)?, key_addr, 0, "d"), "d");
            let len = self.ld(self.ctx.i64(), self.fld(self.lty2(key)?, key_addr, 1, "n"), "n");
            let rt = self.rt("kelp_map_hash")?;
            Ok(self.call(rt.func, rt.ty, &[data, len], "hash"))
        } else {
            let key_ty = self.lty2(key)?;
            let kv = self.ld(key_ty, self.fld(entry_ty, entry, 0, "key"), "key");
            let wide = self.key_to_u64(key, kv)?;
            let rt = self.rt("kelp_map_mix")?;
            Ok(self.call(rt.func, rt.ty, &[wide], "hash"))
        }
    }

    /// Whether the entry's stored key equals the argument `k`.
    fn map_key_eq_stored(
        &self,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
        k: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let (strlen, sty) = self.libc_fn("strlen")?;
            let n = self.call(strlen, sty, &[k], "len");
            let rt = self.rt("kelp_map_key_eq")?;
            Ok(self.call(rt.func, rt.ty, &[key_addr, k, n], "eq"))
        } else {
            let key_ty = self.lty2(key)?;
            let kv = self.ld(key_ty, self.fld(entry_ty, entry, 0, "key"), "key");
            Ok(self.builder.build_icmp(IntCmp::Eq, kv, k, "eq"))
        }
    }

    /// The key value an `insert` stores: a `String` key is copied in from the
    /// `str` view, a scalar key is stored as it arrived.
    fn map_key_value(&self, key: &Ty, k: Value<'a>, slot: Value<'a>) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            self.sret_call("kelp_String__from_str", slot, &[k])?;
            Ok(self.ld(self.lty2(key)?, slot, "key"))
        } else {
            Ok(k)
        }
    }

    /// A copy of the entry's stored key, for `keys()`: a `String` is cloned, a
    /// scalar is loaded.
    fn map_key_clone(
        &self,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
        slot: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            self.sret_call("kelp_String__clone", slot, &[key_addr])?;
            Ok(self.ld(self.lty2(key)?, slot, "key"))
        } else {
            let key_ty = self.lty2(key)?;
            Ok(self.ld(key_ty, self.fld(entry_ty, entry, 0, "key"), "key"))
        }
    }

    /// Free the entry's stored key: a `String` buffer is freed, a scalar owns
    /// nothing.
    fn map_key_drop(
        &self,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
    ) -> Result<(), String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let rt = self.rt("kelp_string_drop")?;
            self.call(rt.func, rt.ty, &[key_addr], "");
        }
        Ok(())
    }

    /// One `Map<K, V>` function set, the mirror of `codegen_c::write_map`.
    /// Entries sit in insertion order in a dense array and a separate
    /// open-addressed index of `entry index + 1` (`0` empty, `SIZE_MAX`
    /// tombstone) does the lookup.
    fn emit_map_bodies(&self, key: &Ty, value: &Ty) -> Result<(), String> {
        let base = map_c_type(key, value);
        let map_ty = self.cty(&Ty::Map(Box::new(key.clone()), Box::new(value.clone())))?;
        let entry_ty = *self
            .map_entries
            .get(&Ty::Map(Box::new(key.clone()), Box::new(value.clone())))
            .ok_or_else(|| no_value("the map entry layout"))?;
        let key_ty = self.lty2(key)?;
        let value_ty = self.lty2(value)?;
        let key_arg = self.map_key_arg(key)?;
        let list_key_ty = self.cty(&Ty::List(Box::new(key.clone())))?;
        let list_value_ty = self.cty(&Ty::List(Box::new(value.clone())))?;
        let ptr = self.pty();
        let (i1t, i8t, i64t) = (self.ctx.i1(), self.ctx.i8(), self.ctx.i64());
        let entry_size = self.layout.abi_size_of(entry_ty);
        let value_drops = needs_drop(value, &self.owned);
        let clonable = list_clonable_elem(value);
        let size_max = u64::MAX;

        // --- new ---
        {
            let _ = self.start(&format!("{base}__new"), "entry")?;
            let m = self.builder.build_alloca(map_ty, "m");
            self.st(self.ctx.const_null(map_ty), m);
            let v = self.ld(map_ty, m, "new");
            self.builder.build_ret(v);
        }

        // --- find(self, k, found_out) ---
        {
            let (f, _) = self.start(&format!("{base}__find"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_p = f.get_param(2);
            self.st(self.ctx.const_bool(false), found_p);
            let icap = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let zero = self.builder.build_icmp(IntCmp::Eq, icap, self.v_i64(0), "noindex");
            let none = self.block(f, "find.none");
            let cont = self.block(f, "find.cont");
            self.builder.build_cond_br(zero, none, cont);
            self.builder.position_at_end(none);
            self.builder.build_ret(self.v_i64(0));
            self.builder.position_at_end(cont);
            let mask = self.builder.build_int_binop(IntBinOp::Sub, icap, self.v_i64(1), "mask");
            let h = self.map_key_hash_arg(key, k)?;
            let i0 = self.builder.build_int_binop(IntBinOp::And, h, mask, "i0");
            let i_slot = self.builder.build_alloca(i64t, "i");
            let tomb_slot = self.builder.build_alloca(i64t, "tomb");
            self.st(i0, i_slot);
            self.st(self.v_i64(size_max), tomb_slot);
            let loop_bb = self.block(f, "find.loop");
            let empty_bb = self.block(f, "find.empty");
            let probe_bb = self.block(f, "find.probe");
            let tomb_bb = self.block(f, "find.tomb");
            let live_bb = self.block(f, "find.live");
            let eq_bb = self.block(f, "find.eq");
            let found_bb = self.block(f, "find.found");
            let next_bb = self.block(f, "find.next");
            self.builder.build_br(loop_bb);
            // loop
            self.builder.position_at_end(loop_bb);
            let i = self.ld(i64t, i_slot, "i");
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i64t, index, i, "slot");
            let slot = self.ld(i64t, slot_addr, "slot");
            let iszero = self.builder.build_icmp(IntCmp::Eq, slot, self.v_i64(0), "iszero");
            self.builder.build_cond_br(iszero, empty_bb, probe_bb);
            self.builder.position_at_end(empty_bb);
            // empty: the first tombstone seen, else this slot
            self.builder.position_at_end(empty_bb);
            let t = self.ld(i64t, tomb_slot, "tomb");
            let istomb = self.builder.build_icmp(IntCmp::Eq, t, self.v_i64(size_max), "istomb");
            let sel = self.builder.build_select(istomb, i, t, "slot");
            self.builder.build_ret(sel);
            // probe: tombstone, or a live entry to compare
            self.builder.position_at_end(probe_bb);
            let istomb2 = self.builder.build_icmp(IntCmp::Eq, slot, self.v_i64(size_max), "istomb");
            self.builder.build_cond_br(istomb2, tomb_bb, live_bb);
            self.builder.position_at_end(tomb_bb);
            let t2 = self.ld(i64t, tomb_slot, "tomb");
            let istomb3 = self.builder.build_icmp(IntCmp::Eq, t2, self.v_i64(size_max), "istomb");
            let keep = self.builder.build_select(istomb3, i, t2, "tomb");
            self.st(keep, tomb_slot);
            self.builder.build_br(next_bb);
            self.builder.position_at_end(live_bb);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let e0 = self.builder.build_int_binop(IntBinOp::Sub, slot, self.v_i64(1), "e");
            let entry = self.gep(entry_ty, entries, e0, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            self.builder.build_cond_br(dead, next_bb, eq_bb);
            self.builder.position_at_end(eq_bb);
            let eq = self.map_key_eq_stored(key, entry_ty, entry, k)?;
            self.builder.build_cond_br(eq, found_bb, next_bb);
            self.builder.position_at_end(found_bb);
            self.st(self.ctx.const_bool(true), found_p);
            self.builder.build_ret(i);
            self.builder.position_at_end(next_bb);
            let i1 = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            let i2 = self.builder.build_int_binop(IntBinOp::And, i1, mask, "i2");
            self.st(i2, i_slot);
            self.builder.build_br(loop_bb);
        }

        // --- rehash(self, want) ---
        {
            let (f, _) = self.start(&format!("{base}__rehash"), "entry")?;
            let self_p = f.get_param(0);
            let want = f.get_param(1);
            let cap_slot = self.builder.build_alloca(i64t, "cap");
            self.st(self.v_i64(8), cap_slot);
            let header = self.loop_header(f, "rehash.cond");
            let cap = self.ld(i64t, cap_slot, "cap");
            let four = self.builder.build_int_binop(IntBinOp::Mul, want, self.v_i64(4), "want4");
            let need = self.builder.build_icmp(IntCmp::Ult, cap, four, "need");
            let body = self.block(f, "rehash.body");
            let exit = self.block(f, "rehash.end");
            self.builder.build_cond_br(need, body, exit);
            self.builder.position_at_end(body);
            let cap2 = self.builder.build_int_binop(IntBinOp::Mul, cap, self.v_i64(2), "cap2");
            self.st(cap2, cap_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let final_cap = self.ld(i64t, cap_slot, "cap");
            let old = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let (free, fty) = self.libc_fn("free")?;
            self.call(free, fty, &[old], "");
            let bytes = self.builder.build_int_binop(IntBinOp::Mul, final_cap, self.v_i64(8), "bytes");
            let (malloc, mty) = self.libc_fn("malloc")?;
            let buf = self.call(malloc, mty, &[bytes], "index");
            self.check_alloc(buf, f)?;
            let (memset, sety) = self.libc_fn("memset")?;
            self.call(memset, sety, &[buf, self.v_i32(0), bytes], "");
            self.st(buf, self.fld(map_ty, self_p, M_INDEX, "index"));
            self.st(final_cap, self.fld(map_ty, self_p, M_INDEX_CAP, "index_cap"));
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_INDEX_USED, "index_used"));
            // Walk the live entries and re-place each in the fresh index.
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let eh = self.loop_header(f, "rehash.walk");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let ebody = self.block(f, "rehash.walk.body");
            let eexit = self.block(f, "rehash.walk.end");
            self.builder.build_cond_br(more, ebody, eexit);
            self.builder.position_at_end(ebody);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "rehash.skip");
            let place = self.block(f, "rehash.place");
            self.builder.build_cond_br(dead, skip, place);
            self.builder.position_at_end(place);
            let h = self.map_key_hash_stored(key, entry_ty, entry)?;
            let mask = self.builder.build_int_binop(IntBinOp::Sub, final_cap, self.v_i64(1), "mask");
            let j_slot = self.builder.build_alloca(i64t, "slot");
            let j0 = self.builder.build_int_binop(IntBinOp::And, h, mask, "j0");
            self.st(j0, j_slot);
            let jh = self.loop_header(f, "rehash.probe");
            let j = self.ld(i64t, j_slot, "j");
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i64t, index, j, "at");
            let holder = self.ld(i64t, slot_addr, "holder");
            let occupied = self.builder.build_icmp(IntCmp::Ne, holder, self.v_i64(0), "occupied");
            let jbody = self.block(f, "rehash.probe.body");
            let jexit = self.block(f, "rehash.probe.end");
            self.builder.build_cond_br(occupied, jbody, jexit);
            self.builder.position_at_end(jbody);
            let jn = self.builder.build_int_binop(IntBinOp::Add, j, self.v_i64(1), "j1");
            let jm = self.builder.build_int_binop(IntBinOp::And, jn, mask, "jm");
            self.st(jm, j_slot);
            self.builder.build_br(jh);
            self.builder.position_at_end(jexit);
            let jf = self.ld(i64t, j_slot, "j");
            let index2 = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let placed = self.gep(i64t, index2, jf, "at");
            let ep1 = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "ep1");
            self.st(ep1, placed);
            let used = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_USED, "used"), "used");
            let used1 = self.builder.build_int_binop(IntBinOp::Add, used, self.v_i64(1), "used1");
            self.st(used1, self.fld(map_ty, self_p, M_INDEX_USED, "used"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(eh);
            self.builder.position_at_end(eexit);
            self.builder.build_ret_void();
        }

        // --- has(self, k) ---
        {
            let (f, _) = self.start(&format!("{base}__has"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            self.call(find, fnty, &[self_p, k, found_slot], "");
            let out = self.ld(i1t, found_slot, "has");
            self.builder.build_ret(out);
        }

        // --- get / get_const ---
        if value.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let bad = self.block(f, "missing");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(missing, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Map.get: key not found")?;
            self.builder.position_at_end(ok);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            let v = self.ld(value_ty, value_ptr, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let bad = self.block(f, "missing");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(missing, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Map.get: key not found")?;
            self.builder.position_at_end(ok);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            self.builder.build_ret(value_ptr);
        }

        // --- get_or(self, k, fallback) ---
        if clonable {
            let (f, _) = self.start(&format!("{base}__get_or"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let fallback = f.get_param(2);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let clone_slot = self.builder.build_alloca(value_ty, "clone");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let fb_slot = self.builder.build_alloca(value_ty, "fallback");
            self.st(fallback, fb_slot);
            let miss = self.block(f, "missing");
            let hit = self.block(f, "hit");
            self.builder.build_cond_br(missing, miss, hit);
            self.builder.position_at_end(miss);
            let out = self.ld(value_ty, fb_slot, "fallback");
            self.builder.build_ret(out);
            self.builder.position_at_end(hit);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            let result = self.clone_value(value_ptr, value, clone_slot)?;
            match value {
                Ty::String => {
                    let rt = self.rt("kelp_string_drop")?;
                    self.call(rt.func, rt.ty, &[fb_slot], "");
                }
                Ty::Bytes => {
                    let rt = self.rt("kelp_bytes_drop")?;
                    self.call(rt.func, rt.ty, &[fb_slot], "");
                }
                _ => {}
            }
            self.builder.build_ret(result);
        }

        // --- remove(self, k) ---
        {
            let (f, _) = self.start(&format!("{base}__remove"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let bad = self.block(f, "missing");
            let ok = self.block(f, "remove");
            self.builder.build_cond_br(missing, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Map.remove: key not found")?;
            self.builder.position_at_end(ok);
            let index0 = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let holder0 = self.ld(i64t, self.gep(i64t, index0, slot, "at"), "holder");
            let e = self.builder.build_int_binop(IntBinOp::Sub, holder0, self.v_i64(1), "e");
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let value_slot = self.fld(entry_ty, entry, E_VALUE, "value");
            let value_ptr = self.ld(ptr, value_slot, "value");
            let out_slot = self.builder.build_alloca(value_ty, "out");
            self.st(self.ld(value_ty, value_ptr, "v"), out_slot);
            let (free, fty) = self.libc_fn("free")?;
            self.call(free, fty, &[value_ptr], "");
            self.st(self.ctx.const_null(ptr), value_slot);
            self.map_key_drop(key, entry_ty, entry)?;
            self.st(self.v_i8(0), self.fld(entry_ty, entry, E_LIVE, "live"));
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i64t, index, slot, "at");
            self.st(self.v_i64(size_max), slot_addr);
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let len1 = self.builder.build_int_binop(IntBinOp::Sub, len, self.v_i64(1), "len1");
            self.st(len1, self.fld(map_ty, self_p, M_LEN, "len"));
            let out = self.ld(value_ty, out_slot, "removed");
            self.builder.build_ret(out);
        }

        // --- insert(self, k, v) ---
        {
            let (f, _) = self.start(&format!("{base}__insert"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let v = f.get_param(2);
            let key_slot = self.builder.build_alloca(key_ty, "key.tmp");
            let icap = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let iused = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_USED, "iused"), "iused");
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let needs = self.builder.build_int_binop(IntBinOp::Add, iused, self.v_i64(1), "need");
            let needs4 = self.builder.build_int_binop(IntBinOp::Mul, needs, self.v_i64(4), "need4");
            let cap3 = self.builder.build_int_binop(IntBinOp::Mul, icap, self.v_i64(3), "cap3");
            let crowded = self.builder.build_icmp(IntCmp::Uge, needs4, cap3, "crowded");
            let noindex = self.builder.build_icmp(IntCmp::Eq, icap, self.v_i64(0), "noindex");
            let grow = self.builder.build_int_binop(IntBinOp::Or, crowded, noindex, "grow");
            let rehash_bb = self.block(f, "insert.rehash");
            let placed = self.block(f, "insert.placed");
            self.builder.build_cond_br(grow, rehash_bb, placed);
            self.builder.position_at_end(rehash_bb);
            let want = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "want");
            let (rehash, rhty) = self.cfn(&format!("{base}__rehash"))?;
            self.call(rehash, rhty, &[self_p, want], "");
            self.builder.build_br(placed);
            self.builder.position_at_end(placed);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let have = self.block(f, "insert.replace");
            let fresh = self.block(f, "insert.fresh");
            self.builder.build_cond_br(found, have, fresh);
            self.builder.position_at_end(have);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            if value_drops {
                self.call_drop(value_ptr, value)?;
            }
            self.st(v, value_ptr);
            self.builder.build_ret_void();
            self.builder.position_at_end(fresh);
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let ecap = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_CAP, "ecap"), "ecap");
            let full = self.builder.build_icmp(IntCmp::Eq, count, ecap, "full");
            let resize = self.block(f, "insert.resize");
            let have_space = self.block(f, "insert.space");
            self.builder.build_cond_br(full, resize, have_space);
            self.builder.position_at_end(resize);
            let is_zero = self.builder.build_icmp(IntCmp::Eq, ecap, self.v_i64(0), "zero");
            let double = self.builder.build_int_binop(IntBinOp::Mul, ecap, self.v_i64(2), "double");
            let new_cap = self.builder.build_select(is_zero, self.v_i64(8), double, "newcap");
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let bytes = self.builder.build_int_binop(IntBinOp::Mul, new_cap, self.v_i64(entry_size), "bytes");
            let (realloc, rty) = self.libc_fn("realloc")?;
            let new_entries = self.call(realloc, rty, &[entries, bytes], "entries");
            self.check_alloc(new_entries, f)?;
            self.st(new_entries, self.fld(map_ty, self_p, M_ENTRIES, "entries"));
            self.st(new_cap, self.fld(map_ty, self_p, M_ENTRY_CAP, "entry_cap"));
            self.builder.build_br(have_space);
            self.builder.position_at_end(have_space);
            let e = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let e1 = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(e1, self.fld(map_ty, self_p, M_ENTRY_COUNT, "entry_count"));
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let key_value = self.map_key_value(key, k, key_slot)?;
            self.st(key_value, self.fld(entry_ty, entry, E_KEY, "key"));
            let value_ptr = self.malloc_for(value_ty)?;
            self.check_alloc(value_ptr, f)?;
            self.st(value_ptr, self.fld(entry_ty, entry, E_VALUE, "value"));
            self.st(v, value_ptr);
            self.st(self.v_i8(1), self.fld(entry_ty, entry, E_LIVE, "live"));
            // Bump `index_used` only when the slot was empty (not a tombstone).
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i64t, index, slot, "at");
            let holder = self.ld(i64t, slot_addr, "holder");
            let was_empty = self.builder.build_icmp(IntCmp::Eq, holder, self.v_i64(0), "wasempty");
            let bump = self.block(f, "insert.bump");
            let done = self.block(f, "insert.done");
            self.builder.build_cond_br(was_empty, bump, done);
            self.builder.position_at_end(bump);
            let used = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_USED, "used"), "used");
            let used1 = self.builder.build_int_binop(IntBinOp::Add, used, self.v_i64(1), "used1");
            self.st(used1, self.fld(map_ty, self_p, M_INDEX_USED, "index_used"));
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            let index2 = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr2 = self.gep(i64t, index2, slot, "at");
            let ep1 = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "ep1");
            self.st(ep1, slot_addr2);
            let len2 = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let len3 = self.builder.build_int_binop(IntBinOp::Add, len2, self.v_i64(1), "len1");
            self.st(len3, self.fld(map_ty, self_p, M_LEN, "len"));
            self.builder.build_ret_void();
        }

        // --- clear(self) ---
        {
            let (f, _) = self.start(&format!("{base}__clear"), "entry")?;
            let self_p = f.get_param(0);
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "clear.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "clear.body");
            let exit = self.block(f, "clear.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "clear.skip");
            let free_bb = self.block(f, "clear.free");
            self.builder.build_cond_br(dead, skip, free_bb);
            self.builder.position_at_end(free_bb);
            let value_slot = self.fld(entry_ty, entry, E_VALUE, "value");
            let value_ptr = self.ld(ptr, value_slot, "value");
            if value_drops {
                self.call_drop(value_ptr, value)?;
            }
            let (free, fty) = self.libc_fn("free")?;
            self.call(free, fty, &[value_ptr], "");
            self.st(self.ctx.const_null(ptr), value_slot);
            self.map_key_drop(key, entry_ty, entry)?;
            self.st(self.v_i8(0), self.fld(entry_ty, entry, E_LIVE, "live"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_ENTRY_COUNT, "entry_count"));
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_LEN, "len"));
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_INDEX_USED, "index_used"));
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let noindex = self.builder.build_icmp(IntCmp::Eq, index, self.ctx.const_null(ptr), "noindex");
            let clear_index = self.block(f, "clear.index");
            let end = self.block(f, "clear.ret");
            self.builder.build_cond_br(noindex, end, clear_index);
            self.builder.position_at_end(clear_index);
            let icap = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let bytes = self.builder.build_int_binop(IntBinOp::Mul, icap, self.v_i64(8), "bytes");
            let (memset, sety) = self.libc_fn("memset")?;
            self.call(memset, sety, &[index, self.v_i32(0), bytes], "");
            self.builder.build_br(end);
            self.builder.position_at_end(end);
            self.builder.build_ret_void();
        }

        // --- len / is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__len"), "entry")?;
            let n = self.ld(i64t, self.fld(map_ty, f.get_param(0), M_LEN, "len"), "len");
            self.builder.build_ret(n);
        }
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let n = self.ld(i64t, self.fld(map_ty, f.get_param(0), M_LEN, "len"), "len");
            let e = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
            self.builder.build_ret(e);
        }

        // --- keys(self) ---
        {
            let (f, _) = self.start(&format!("{base}__keys"), "entry")?;
            let self_p = f.get_param(0);
            let out_slot = self.builder.build_alloca(list_key_ty, "out");
            let key_slot = self.builder.build_alloca(key_ty, "key.tmp");
            self.st(self.ctx.const_null(list_key_ty), out_slot);
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let list_reserve = self.cfn(&format!("kelp_List__{}__reserve", ty_mangle(key)))?;
            self.call(list_reserve.0, list_reserve.1, &[out_slot, len], "");
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "keys.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "keys.body");
            let exit = self.block(f, "keys.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "keys.skip");
            let append = self.block(f, "keys.append");
            self.builder.build_cond_br(dead, skip, append);
            self.builder.position_at_end(append);
            let kv = self.map_key_clone(key, entry_ty, entry, key_slot)?;
            let olen = self.ld(i64t, self.fld(list_key_ty, out_slot, L_LEN, "olen"), "olen");
            let odata = self.ld(ptr, self.fld(list_key_ty, out_slot, L_DATA, "odata"), "odata");
            let dst = self.gep(key_ty, odata, olen, "dst");
            self.st(kv, dst);
            let olen1 = self.builder.build_int_binop(IntBinOp::Add, olen, self.v_i64(1), "olen1");
            self.st(olen1, self.fld(list_key_ty, out_slot, L_LEN, "len"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(list_key_ty, out_slot, "keys");
            self.builder.build_ret(out);
        }

        // --- values(self) ---
        if clonable {
            let (f, _) = self.start(&format!("{base}__values"), "entry")?;
            let self_p = f.get_param(0);
            let out_slot = self.builder.build_alloca(list_value_ty, "out");
            let clone_slot = self.builder.build_alloca(value_ty, "value.tmp");
            self.st(self.ctx.const_null(list_value_ty), out_slot);
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let list_reserve = self.cfn(&format!("kelp_List__{}__reserve", ty_mangle(value)))?;
            self.call(list_reserve.0, list_reserve.1, &[out_slot, len], "");
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "values.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "values.body");
            let exit = self.block(f, "values.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "values.skip");
            let append = self.block(f, "values.append");
            self.builder.build_cond_br(dead, skip, append);
            self.builder.position_at_end(append);
            let value_ptr = self.ld(ptr, self.fld(entry_ty, entry, E_VALUE, "value"), "value");
            let vv = self.clone_value(value_ptr, value, clone_slot)?;
            let olen = self.ld(i64t, self.fld(list_value_ty, out_slot, L_LEN, "olen"), "olen");
            let odata = self.ld(ptr, self.fld(list_value_ty, out_slot, L_DATA, "odata"), "odata");
            let dst = self.gep(value_ty, odata, olen, "dst");
            self.st(vv, dst);
            let olen1 = self.builder.build_int_binop(IntBinOp::Add, olen, self.v_i64(1), "olen1");
            self.st(olen1, self.fld(list_value_ty, out_slot, L_LEN, "len"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(list_value_ty, out_slot, "values");
            self.builder.build_ret(out);
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(
                &drop_fn_name(&Ty::Map(Box::new(key.clone()), Box::new(value.clone()))),
                "entry",
            )?;
            let self_p = f.get_param(0);
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "drop.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "drop.body");
            let exit = self.block(f, "drop.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "drop.skip");
            let free_bb = self.block(f, "drop.free");
            self.builder.build_cond_br(dead, skip, free_bb);
            self.builder.position_at_end(free_bb);
            let value_ptr = self.ld(ptr, self.fld(entry_ty, entry, E_VALUE, "value"), "value");
            if value_drops {
                self.call_drop(value_ptr, value)?;
            }
            let (free, fty) = self.libc_fn("free")?;
            self.call(free, fty, &[value_ptr], "");
            self.map_key_drop(key, entry_ty, entry)?;
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            self.call(free, fty, &[entries], "");
            self.call(free, fty, &[index], "");
            self.st(self.ctx.const_null(map_ty), self_p);
            self.builder.build_ret_void();
        }
        let _ = (value_ty, list_value_ty, list_key_ty, key_arg);
        Ok(())
    }

    /// The address of the value cell an entry holds: `entries[e].value`, where
    /// `slot` is the index-slot result `find` returned (`entries[slot - 1]`).
    fn map_value_ptr(
        &self,
        map_ty: Type<'a>,
        entry_ty: Type<'a>,
        self_p: Value<'a>,
        slot: Value<'a>,
    ) -> Result<Value<'a>, String> {
        // `slot` is the *index* slot `find` returned; the entry it names is
        // `index[slot] - 1`, exactly the C `self->index[slot] - 1`.
        let index = self.ld(self.pty(), self.fld(map_ty, self_p, M_INDEX, "index"), "index");
        let holder = self.ld(self.ctx.i64(), self.gep(self.ctx.i64(), index, slot, "at"), "holder");
        let e = self.builder.build_int_binop(IntBinOp::Sub, holder, self.v_i64(1), "e");
        let entries = self.ld(self.pty(), self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
        let entry = self.gep(entry_ty, entries, e, "entry");
        let value_slot = self.fld(entry_ty, entry, E_VALUE, "value");
        Ok(self.ld(self.pty(), value_slot, "value"))
    }
}

/// The map's fields, in the order `write_container_typedef` declares them, and
/// one entry's three, in `write_map`'s order.
const M_ENTRIES: u32 = 0;
const M_ENTRY_COUNT: u32 = 1;
const M_ENTRY_CAP: u32 = 2;
const M_LEN: u32 = 3;
const M_INDEX: u32 = 4;
const M_INDEX_CAP: u32 = 5;
const M_INDEX_USED: u32 = 6;
const E_KEY: u32 = 0;
const E_VALUE: u32 = 1;
const E_LIVE: u32 = 2;

/// The C translation unit carrying the compiler-supplied `extern fun` bodies
/// this program reaches and the capability check's own C half (MK7 S3,
/// plan §3.2 and §5), or `None` when it needs neither.
///
/// The bodies stay C. They are the one place an unportable primitive (`malloc`,
/// `clock_gettime`, a raw `read` on a descriptor) is allowed to live, and their
/// text is what the C backend compiles into each generated program. Every byte
/// of that text here comes from the same emitters the C backend uses:
/// `codegen_c.rs::write_extern` writes the signature and the body, and
/// `runtime_prototypes` declares the Kelp half of the runtime
/// (`std/rt`) that one body calls. A second copy of either would be the drift
/// this slice has to avoid.
///
/// # Why this object is generated per program, not compiled once
///
/// MK7 §3.2 compiles the *runtime* once, in `build.rs`, because a fixed object
/// serves every program. The extern bodies cannot be served that way, for two
/// reasons that are properties of the bodies themselves:
///
/// * a body may name a symbol that only exists when its own module is loaded.
///   `std.path.normalize_raw` calls `kelp_std__rt__path_normalize`, a Kelp
///   function the resolver injects only for a program that imports `std/path`.
///   A single object linked into every binary would carry that undefined
///   reference into programs that never mention `path.normalize`, and the link
///   would fail for all of them. Nothing in the object could be dropped by the
///   linker either: the object is not an archive and its sections are not
///   garbage-collected by default.
/// * a body may name a monomorphized type. `process.spawn_raw` takes a
///   `List<String>`, so the C signature `write_extern` writes for it names
///   `kelp_List__String`, a typedef that exists only for the program that uses
///   it.
///
/// Emitting exactly `program.functions`' extern set, which is the set the C
/// backend emits, keeps the object's undefined references to exactly the symbols
/// the program has. That is the property a link needs, and it is the reason this
/// is not a build-time object like `kelp_rt.o`.
///
/// # The capability glue
///
/// `runtime/cap_glue.c` is the same text the C backend splices into a generated
/// program that contains a gated call: the deny path, the buffered plumbing and
/// the allowlist loops. It is emitted here, in the same object, for the same
/// reason the extern bodies are: its checks call the Kelp matcher (`std/rt`),
/// which exists only for a program that holds a grant, and it is C text that
/// must not be re-derived (MK7 §3.2). The one difference is linkage: in the C
/// backend the call sites are in the same translation unit and the glue is
/// `static`, while here the callers are IR, so `#define static` around the
/// include is what gives the check functions the external linkage the IR
/// declares them with (`declare_cap_fns`).
///
/// The allowlist arrays are emitted here too, as C data, with the C backend's
/// own escaper (`codegen_c.rs::c_string_literal`). They are not `static`: the IR
/// names them by symbol (`cap_array_symbol`), and a static array would have no
/// symbol to name.
fn program_c_source(program: &Program) -> Option<String> {
    let externs: Vec<&Function> = program.functions.iter().filter(|f| f.extern_).collect();
    // `program.gated` is non-empty exactly when the program contains a gated
    // call site, which is the condition the C backend emits the glue under.
    let cap_glue = !program.gated.is_empty();
    if externs.is_empty() && !cap_glue {
        return None;
    }
    let mut out = String::new();
    out.push_str(
        "/* Generated by the llvm backend: the compiler-supplied `extern fun`\n   \
         bodies this program reaches (MK7 S3, the record in plan/done.md).\n   \
         The bodies are C text in `kelpc/src/externs.rs`; the signatures come\n   \
         from `codegen_c.rs::write_extern`, so this file is the C backend's\n   \
         copy of the same functions. */\n",
    );
    out.push_str(crate::codegen_c::C_HEADERS);

    // The builtin layouts, field for field the structs `runtime/kelp_rt.c`
    // defines (MK7 §3.3). Only the types an extern signature names are
    // repeated, and `layout_tests::the_extern_prelude_matches_the_c_runtime`
    // pins them against that file.
    out.push_str(
        "/* The builtin heap layouts of `runtime/kelp_rt.c`: the extern bodies\n   \
         read and return these by value, so their field order and widths have to\n   \
         be the runtime's own (MK7 3.3). */\n\
         typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_Bytes;\n\
         typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_String;\n\
         typedef struct { const char* ptr; size_t len; } kelp_View;\n",
    );

    // A body whose signature names a monomorphized container needs that
    // typedef (`process.spawn_raw`'s `List<String>`), and the C backend writes
    // it with the same function. The set is collected from the signatures
    // rather than from the whole program: a container the externs never name is
    // not this file's business.
    let mut containers: Vec<Ty> = Vec::new();
    for f in &externs {
        collect_container_tys(&f.ret, &mut containers);
        for (_, ty) in &f.params {
            collect_container_tys(ty, &mut containers);
        }
    }
    if !containers.is_empty() {
        out.push('\n');
        for ty in &containers {
            crate::codegen_c::write_container_typedef(&mut out, ty);
        }
    }

    // The runtime helpers and globals the bodies call and read, declared and
    // never defined here: the definitions are in the object `build.rs` compiles
    // from `runtime/kelp_rt.c`, whose `#define static` is what gives them the
    // external linkage these declarations name. `cc` is invoked with
    // `-Werror=implicit-function-declaration`, so a body that reached a helper
    // missing from this list would fail the build rather than call it through an
    // invented signature.
    out.push_str(
        "\n/* The runtime's own functions and process globals, defined in the\n   \
         object `build.rs` compiles from `runtime/kelp_rt.c`. */\n\
         void kelp_panic(const char* msg);\n\
         void kelp_panic_at(const char* msg, int p);\n\
         void kelp_panic_errno(const char* msg);\n\
         void kelp_panic_gai(const char* msg, int rc);\n\
         void kelp_io_flush_stdio(int fd);\n\
         bool kelp_utf8_valid(const unsigned char* p, size_t n);\n\
         kelp_Bytes kelp_bytes_alloc_raw(size_t n);\n\
         void kelp_bytes_drop(kelp_Bytes* b);\n\
         kelp_String kelp_String__empty(void);\n\
         kelp_String kelp_String__from_str(const char* text);\n\
         kelp_String kelp_String__from_bytes(kelp_Bytes b);\n\
         extern int kelp_argc;\n\
         extern char** kelp_argv;\n",
    );

    // The Kelp half of the runtime (`std/rt`), declared with the C backend's
    // own prototype writer. Only `path.normalize` calls into it today, and the
    // resolver injects the module exactly when a program can reach that call, so
    // the reference always has a definition. The list is the whole module rather
    // than the one function: it is one emitter, and an unused declaration costs
    // nothing.
    let prototypes = crate::codegen_c::runtime_prototypes(program);
    if !prototypes.is_empty() {
        out.push('\n');
        out.push_str(&prototypes);
    }

    for f in &externs {
        out.push('\n');
        crate::codegen_c::write_extern(&mut out, f);
    }

    if cap_glue {
        // Each module's granted patterns, as constant data, exactly as
        // `codegen_c.rs::generate` writes them. An empty pattern list is `{
        // NULL }` rather than omitted: a module can hold a domain with no
        // patterns, and a call that names a resource still has to have an array
        // to deny against.
        out.push_str("\n/* capability allowlists, compiled in per module */\n");
        for g in &program.caps {
            out.push_str(&format!(
                "const char* {}[] = {{ {}NULL }};\n",
                cap_array_symbol(&g.module, &g.domain),
                g.patterns
                    .iter()
                    .map(|p| format!("{}, ", crate::codegen_c::c_string_literal(p)))
                    .collect::<String>()
            ));
        }
        out.push_str("\n/* The capability check's C half, the text `codegen_c.rs`\n   \
            splices into a generated program. `#define static` gives its\n   \
            functions the external linkage the IR calls them with; the C\n   \
            backend's own copy stays static, where the call sites are in the\n   \
            same translation unit. */\n#define static\n");
        out.push_str(crate::codegen_c::CAP_GLUE);
        out.push_str("\n#undef static\n");
    }
    Some(out)
}

/// Every container type a `tir` type names, outermost first.
///
/// Used by [`program_c_source`] to emit the container typedefs an extern
/// signature names. It walks the type's own subterms because a `Box<List<T>>`
/// names both typedefs, and it does not walk into a `datac` (an extern takes
/// one by borrow, and the C body sees its forward-declared tag at most).
fn collect_container_tys(ty: &Ty, out: &mut Vec<Ty>) {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => {
            if !out.contains(ty) {
                out.push(ty.clone());
            }
            collect_container_tys(elem, out);
        }
        Ty::Map(key, value) => {
            if !out.contains(ty) {
                out.push(ty.clone());
            }
            collect_container_tys(key, out);
            collect_container_tys(value, out);
        }
        Ty::Array(elem, _) => {
            if !out.contains(ty) {
                out.push(ty.clone());
            }
            collect_container_tys(elem, out);
        }
        Ty::Ref { inner, .. } => collect_container_tys(inner, out),
        _ => {}
    }
}

/// Whether a `tir` type is lowered to an LLVM aggregate rather than a scalar or
/// a pointer. Only an aggregate can be too large to cross a C boundary by value,
/// which is the one thing the verifier cannot see (BUG-vN4tR).
fn is_aggregate_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Bytes
            | Ty::String
            | Ty::View
            | Ty::Struct(_)
            | Ty::List(_)
            | Ty::Box(_)
            | Ty::Map(..)
            | Ty::Array(..)
            | Ty::Closure { .. }
    )
}

/// Whether a value of type `ty`, lowered to `llvm_ty`, is an aggregate over two
/// eightbytes: too large for the SysV ABI to pass in a register pair, so it can
/// cross a C boundary only through a pointer (BUG-vN4tR, MK7 §3.3).
///
/// This is the one name for that rule, and the two places the backend spells a
/// type across a C boundary both ask it, so they cannot drift:
///
/// * `declare_extern`: a parameter that is one is refused by name, a return that
///   is one is declared `void` with the destination as argument 0 (`sret`).
/// * `rt_arg_value`: a call argument that is one is staged into a slot and the
///   slot's address is what crosses.
///
/// `declare_runtime` spells its own 24-byte returns the same way with the `sret`
/// flag, and the `kelp_String__from_bytes_ptr`/`kelp_String__into_bytes_ptr`
/// adapters exist so no 24-byte argument needs the by-value form at all.
fn crosses_c_boundary_by_value(ty: &Ty, llvm_ty: Type<'_>, layout: &DataLayout) -> bool {
    is_aggregate_ty(ty) && layout.abi_size_of(llvm_ty) > 16
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
/// `kelp_String__into_bytes` is deliberately absent: it takes a 24-byte struct
/// *by value*, and LLVM's backend passes such an argument in registers while the
/// SysV ABI puts it on the stack, so a call to one would not cross correctly.
/// The runtime object carries pointer-spelled adapters for both directions
/// (`kelp_String__from_bytes_ptr`, `kelp_String__into_bytes_ptr`, BUG-vN4tR),
/// registered below under the builtin's own mangled name so a call site reaches
/// them through the same lookup every other runtime method uses. Because every
/// `String` construction in the stdlib goes through `from_bytes`, that direction
/// is not left out.
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
    // The map key helpers `write_map` calls: `#define static` in `build.rs`
    // gives them external linkage, so the IR declares them rather than
    // re-deriving the hash and the equality (MK7 §3.2, §3.3). `kelp_map_hash`
    // is FNV-1a over the key's bytes; `kelp_map_mix` is the 64-bit finalizer a
    // scalar key goes through; `kelp_map_key_eq` compares a stored `String`
    // against a `str` view length-aware.
    decl(&mut out, "kelp_map_hash", i64, &[ptr, i64], true, false);
    decl(&mut out, "kelp_map_mix", i64, &[i64], true, false);
    decl(&mut out, "kelp_map_key_eq", i1, &[ptr, ptr, i64], true, false);
    // `String#from_bytes`'s C entry point takes a 24-byte `kelp_Bytes` by value,
    // which an IR call cannot cross (BUG-vN4tR, above). The runtime object's
    // adapter spells that argument as a pointer, and it returns the `String`
    // through the hidden first argument clang gives a 24-byte return.
    {
        let ty = ctx.fn_type(void, &[ptr, ptr], false);
        let func = module.add_function("kelp_String__from_bytes_ptr", ty);
        out.insert(
            // The key is the *builtin's* mangled name, so `emit_call` finds it
            // with the same `symbol_name(callee)` lookup it uses for every
            // other runtime method; the function it calls is the adapter.
            symbol_name("String#from_bytes"),
            RtFn {
                func,
                ty,
                yields: true,
                ret: Some(string),
                sret: true,
            },
        );
    }
    // `String#into_bytes` has the opposite shape and the same problem: its C
    // form takes a 24-byte `kelp_String` *by value* and returns a 24-byte
    // `kelp_Bytes`. The adapter takes the receiver by pointer and returns the
    // buffer through the hidden first argument, and `rt_arg_value` stages the
    // receiver into a slot of its own, so the call never passes a 24-byte
    // aggregate by value in either direction (BUG-vN4tR, MK7 §3.3).
    {
        let ty = ctx.fn_type(void, &[ptr, ptr], false);
        let func = module.add_function("kelp_String__into_bytes_ptr", ty);
        out.insert(
            symbol_name("String#into_bytes"),
            RtFn {
                func,
                ty,
                yields: true,
                ret: Some(bytes),
                sret: true,
            },
        );
    }
    out
}

/// Map a `tir` type to its LLVM type. The scalar subset, the builtin heap types,
/// `str`, a `datac` `structs` holds a type for, and a container `containers`
/// holds a named struct for.
fn llvm_type<'a>(
    ctx: &'a Context,
    structs: &HashMap<String, Type<'a>>,
    containers: &HashMap<Ty, Type<'a>>,
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
        // A container is a named struct here; a borrow of one is its address,
        // which is what every method's `self` and every container parameter is.
        Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..) => *containers
            .get(ty)
            .ok_or_else(|| no_value(&format!("the container '{}'", ty.display())))?,
        Ty::Ref { inner, .. }
            if matches!(**inner, Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..)) =>
        {
            ctx.opaque_ptr()
        }
        // Every other borrow is one pointer too (S2F): a `mut i64`
        // parameter, an `&i32` local, a field borrow, a borrow a container
        // method returns. A borrow of a function value or of a closure is the
        // same pointer, and it is what a closure parameter is (S2E): the
        // parameter rule borrows a non-`Copy` closure, so the fat pointer's
        // address is what a callee projects through.
        Ty::Ref { .. } => ctx.opaque_ptr(),
        // A plain function value is the function's address (S2E): one `ptr`,
        // `Copy`, owning nothing.
        Ty::Fn { .. } => ctx.opaque_ptr(),
        // A closure is the fat pointer `{ fn, env, drop }`, one identified
        // struct per signature and so a named type here like a container
        // (S2E).
        Ty::Closure { .. } => *containers
            .get(ty)
            .ok_or_else(|| no_value(&format!("the closure '{}'", ty.display())))?,
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
    containers: &HashMap<Ty, Type<'a>>,
    ty: &Ty,
) -> Result<Type<'a>, String> {
    if *ty == Ty::Unit {
        Ok(ctx.void())
    } else {
        llvm_type(ctx, structs, containers, ty)
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

/// Whether a type is a borrow: a `Ty::Ref` of anything. The binding holds one
/// pointer, so a place read has to load it rather than hand out the slot; the
/// pointee does not matter, because `llvm_type` gives every `Ref` the same
/// `ptr` (S2F). `&str` is included, since a `&str` binding holds the `char*`
/// itself.
fn is_borrow_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { .. })
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

/// Whether a block's tail diverges instead of producing a value. `panic` is the
/// one expression the frontend puts there (`infer.rs` accepts it against any
/// declared return type), and the C backend special-cases the same call in
/// `write_function_body`; without this a value-returning function whose tail is
/// a `panic` asks the unit-typed call for a value and reports an internal error.
fn is_diverging_tail(tail: &Expr) -> bool {
    matches!(&tail.kind, ExprKind::Call { callee, .. } if callee == "panic")
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
            | ExprKind::Capture { .. }
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

/// The mangled symbol of the monomorphized function a builtin container method
/// resolves to, mirroring `codegen_c.rs::call_value`'s name construction: the
/// element type comes from the receiver argument (or, for the associated
/// `List.of(v)`/`Box.of(v)`, from `v` itself, and for `empty`/`Map.new()` from
/// the call's own result type). `get` has two spellings, by value for a `Copy`
/// element and by pointer for an owned one, decided by the call's own result
/// type. `None` for a call that is not a container method.
fn container_symbol(callee: &str, args: &[Expr], ret: &Ty) -> Option<String> {
    use crate::types::{array_c_type, array_elem_of, array_len_of, box_c_type, box_elem_of};
    for (prefix, name_of, elem_of) in [
        (
            "List#",
            crate::types::list_c_type as fn(&Ty) -> String,
            crate::types::list_elem_of as fn(&Ty) -> Option<&Ty>,
        ),
        (
            "Box#",
            box_c_type as fn(&Ty) -> String,
            box_elem_of as fn(&Ty) -> Option<&Ty>,
        ),
    ] {
        let Some(op) = callee.strip_prefix(prefix) else {
            continue;
        };
        let elem = if op == "of" {
            args.first().map(|a| a.ty.clone())
        } else if op == "empty" || op == "new" {
            elem_of(ret).cloned()
        } else {
            args.first().and_then(|a| elem_of(&a.ty)).cloned()
        };
        if let Some(elem) = elem {
            let base = name_of(&elem);
            let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
                "get_const"
            } else {
                op
            };
            return Some(format!("{base}__{suffix}"));
        }
    }
    if let Some(op) = callee.strip_prefix("Array#") {
        let e = args.first()?;
        let elem = array_elem_of(&e.ty)?.clone();
        let len = array_len_of(&e.ty)?;
        let base = array_c_type(&elem, len);
        let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
            "get_const"
        } else {
            op
        };
        return Some(format!("{base}__{suffix}"));
    }
    if let Some(op) = callee.strip_prefix("Map#") {
        let (value, key) = if op == "new" {
            (
                crate::types::map_elem_of(ret).cloned(),
                crate::types::map_key_of(ret).cloned(),
            )
        } else {
            let e = args.first()?;
            (
                crate::types::map_elem_of(&e.ty).cloned(),
                crate::types::map_key_of(&e.ty).cloned(),
            )
        };
        if let (Some(value), Some(key)) = (value, key) {
            let base = map_c_type(&key, &value);
            let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
                "get_const"
            } else {
                op
            };
            return Some(format!("{base}__{suffix}"));
        }
    }
    None
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

/// The refusal message for a construct the lowering does not handle. `what` is
/// the construct's name, quoted when it is a language word (`'enum'`) and a
/// phrase when it is a shape; `stage` is the MK7 stage that lowered it. Every
/// stage that has landed still passes its label, so the string reads as
/// history; a refusal that fires today names a shape the frontend does not
/// currently produce, so it says the shortfall is the backend's rather than
/// sending the caller to a backend they cannot choose.
fn refuse(what: &str, stage: &str) -> String {
    format!("the llvm backend cannot lower {what} yet (MK7 {stage}); this is a compiler defect")
}

/// Refuse a `datac` field or an `enum` payload field this slice cannot lay out,
/// with the construct named.
///
/// A `String`/`Bytes` field, a field of an owned aggregate and a field that
/// names a `datac` or an `enum` are laid out (MK7 §3.1, §3.4), and so are a
/// container, an array and a function value or closure (S2D, S2E); everything
/// else is named by its own construct. The `str`/`View` rows are unreachable
/// from the frontend (`types::is_borrowed_field_ty` rejects them) but are
/// refused here rather than mis-lowered if they ever appear.
fn refuse_field(ty: &Ty) -> Result<(), String> {
    match ty {
        Ty::Bytes | Ty::String => Ok(()),
        // A container field is laid out now (S2D): a `List`/`Box`/`Map` holds its
        // contents through a pointer, an array inline, and both are complete
        // types by the time the `datac` that names one is given a body.
        Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..) => Ok(()),
        // A borrowed field never reaches codegen at all: the frontend refuses
        // one first (`types::is_borrowed_field_ty`, which names `&str`, `View`
        // and a reference). These three rows are therefore unreachable, and are
        // refused here rather than mis-lowered if one ever appears.
        // `refuse_field` has to stay total over `Ty`, so the rows remain.
        Ty::Str => Err(refuse("'a 'str' field'", "S2F")),
        Ty::View => Err(refuse("'a 'View' field'", "S2F")),
        Ty::Ref { .. } => Err(refuse("'a reference field'", "S2F")),
        // A function value is a `ptr` field and a closure is the fat pointer's
        // struct, laid out by the container pass before this one (S2E).
        Ty::Fn { .. } | Ty::Closure { .. } => Ok(()),
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
/// is the container slice (S2D); a function value, a closure and a borrow of
/// either are lowered from S2E on, so those rows are unreachable and are kept
/// only so the match stays total without a panic.
fn refuse_type(ty: &Ty) -> String {
    let (what, stage) = match ty {
        Ty::Struct(_) => ("'datac'", "S2B"),
        Ty::Bytes => ("'Bytes'", "S2B"),
        Ty::String => ("'String'", "S2B"),
        Ty::List(_) => ("'List'", "S2D"),
        Ty::Box(_) => ("'Box'", "S2D"),
        Ty::Map(..) => ("'Map'", "S2D"),
        Ty::Array(..) => ("'an array'", "S2D"),
        Ty::Ref { .. } | Ty::Fn { .. } | Ty::Closure { .. } => {
            return no_value(&format!("the type '{}'", ty.display()))
        }
        Ty::Str | Ty::View => ("a borrowed view", "S2D"),
        Ty::Unit => ("'unit'", "S1"),
        other => return refuse(&format!("the type '{}'", other.display()), "S1"),
    };
    refuse(what, stage)
}

/// The internal-error text for a value position the lowering expected to hold a
/// value and did not. It is an `Err`, not a panic: even an internal mistake
/// leaves the user with a named line rather than a backtrace.
fn no_value(what: &str) -> String {
    format!("internal error: the llvm backend has no value for {what} (MK7 S1)")
}

#[cfg(test)]
mod layout_tests {
    use super::program_c_source;
    use crate::lexer::Span;
    use crate::llvm_ffi::{Context, Module};
    use crate::tir::{Block, Function, Program};
    use crate::types::Ty;

    /// C comments removed, so a struct laid out over several commented lines in
    /// `runtime/kelp_rt.c` can be compared with the one-line typedef the extern
    /// prelude writes.
    fn strip_c_comments(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("/*") {
            out.push_str(&rest[..start]);
            match rest[start..].find("*/") {
                Some(end) => rest = &rest[start + end + 2..],
                None => {
                    rest = "";
                    break;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// The extern prelude repeats the builtin struct layouts and declares every
    /// runtime helper the bodies call (MK7 §3.3). The C compiler checks a
    /// prototype's arity, not its agreement with a definition in another
    /// translation unit, so the agreement is pinned here by reading both texts:
    /// each repeated typedef is the same field-for-field shape, and each declared
    /// helper and process global is one `runtime/kelp_rt.c` actually defines.
    #[test]
    fn the_extern_prelude_matches_the_c_runtime() {
        let program = Program {
            structs: Vec::new(),
            enums: Vec::new(),
            pds: Vec::new(),
            functions: vec![Function {
                name: "std.mem.alloc_raw".to_string(),
                params: vec![("n".to_string(), Ty::Usize)],
                ret: Ty::Bytes,
                body: Block {
                    stmts: Vec::new(),
                    tail: None,
                },
                // A span of zero: this declaration is synthesized for the
                // test, and the DWARF skeleton is built from a real one.
                span: Span::new(0, 0),
                module: "std/mem".to_string(),
                extern_: true,
                env: None,
            }],
            caps: Vec::new(),
            gated: Default::default(),
        };
        let source = program_c_source(&program).expect("one extern fun is one body");

        // The layout text, written once in the prelude and once in the runtime.
        let runtime = strip_c_comments(include_str!("../runtime/kelp_rt.c"));
        let squeeze = |text: &str| text.split_whitespace().collect::<String>();
        let runtime = squeeze(&runtime);
        let layouts = [
            "typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_Bytes;",
            "typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_String;",
            "typedef struct { const char* ptr; size_t len; } kelp_View;",
        ];
        for want in layouts {
            assert!(
                source.contains(want),
                "the extern prelude does not declare '{want}':\n{source}"
            );
            assert!(
                runtime.contains(&squeeze(want)),
                "'runtime/kelp_rt.c' does not define '{want}'"
            );
        }

        // Every declared helper and global: named here so a body that starts
        // calling a new one has to add it to both places at once.
        let declarations = [
            "void kelp_panic(const char* msg);",
            "void kelp_panic_at(const char* msg, int p);",
            "void kelp_panic_errno(const char* msg);",
            "void kelp_panic_gai(const char* msg, int rc);",
            "void kelp_io_flush_stdio(int fd);",
            "bool kelp_utf8_valid(const unsigned char* p, size_t n);",
            "kelp_Bytes kelp_bytes_alloc_raw(size_t n);",
            "void kelp_bytes_drop(kelp_Bytes* b);",
            "kelp_String kelp_String__empty(void);",
            "kelp_String kelp_String__from_str(const char* text);",
            "kelp_String kelp_String__from_bytes(kelp_Bytes b);",
        ];
        for decl in declarations {
            assert!(
                source.contains(decl),
                "the extern prelude does not declare '{decl}':\n{source}"
            );
            let name = decl
                .split_whitespace()
                .nth(1)
                .and_then(|rest| rest.split('(').next())
                .expect("a declaration has a name");
            assert!(
                runtime.contains(&format!("{name}(")),
                "'runtime/kelp_rt.c' does not define '{name}'"
            );
        }
        // The two process globals: `build.rs` strips `static`, so the object
        // defines them with the external linkage the prelude declares.
        for decl in ["extern int kelp_argc;", "extern char** kelp_argv;"] {
            assert!(
                source.contains(decl),
                "the extern prelude does not declare '{decl}':\n{source}"
            );
        }
        for want in ["int kelp_argc = 0;", "char** kelp_argv = NULL;"] {
            assert!(
                runtime.contains(&squeeze(want)),
                "'runtime/kelp_rt.c' does not define '{want}'"
            );
        }
    }

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

    /// Every `extern fun` the compiler ships, compiled against the prelude the
    /// LLVM backend emits for it (MK7 S3A).
    ///
    /// The corpus cannot reach this: the modules whose bodies are the widest
    /// (`std/fs`, `std/net`, `std/process`, `std/io`) each carry a `pd`, so the
    /// backend refuses those programs before it would emit their externs file.
    /// This test runs the front end only, which `pd` does not disturb, so the
    /// program is the whole shipped extern surface at once and `cc` is the
    /// judge: a body that calls a runtime helper the prelude forgot, or one that
    /// needs a header it does not include, fails here rather than in the slice
    /// that lowers `pd`.
    #[test]
    fn every_shipped_extern_body_compiles_against_the_prelude() {
        use crate::diag::SourceMap;

        let modules = [
            "std/mem",
            "std/str",
            "std/path",
            "std/math",
            "std/fs",
            "std/time",
            "std/rand",
            "std/env",
            "std/fmt",
            "std/io",
            "std/process",
            "std/net",
            "std/encoding",
            "std/list",
            "std/option",
            "std/result",
        ];
        let mut entry_source = String::new();
        for id in modules {
            entry_source.push_str(&format!("impl \"{id}\"\n"));
        }
        entry_source.push_str("\nfun main() {\n    print(1)\n}\n");

        let dir = std::env::temp_dir().join(format!(
            "kelp_extern_prelude_{}_{}",
            std::process::id(),
            modules.len()
        ));
        std::fs::create_dir_all(&dir).expect("a scratch directory for the entry file");
        let entry = dir.join("main.kp");
        std::fs::write(&entry, entry_source).expect("the entry file");

        let mut sources = SourceMap::new();
        let loaded = crate::resolve::load(&entry, &mut sources)
            .expect("an import of every shipped stdlib module resolves");
        let program = crate::infer::check_program(&loaded.program, &loaded.project)
            .expect("the shipped stdlib type-checks");
        let source = program_c_source(&program).expect("the stdlib declares extern funs");

        if let Err(e) = crate::driver::compile_c_object(
            &source,
            &dir.join("all.externs.c"),
            &dir.join("all.externs.o"),
            crate::opt::OptLevel::DEFAULT,
        ) {
            panic!(
                "a shipped extern body does not compile against the prelude: {}",
                e.message
            );
        }
    }

    /// The container headers the module builds must match the C typedefs in
    /// `codegen_c.rs::write_container_typedef` field for field, because a
    /// container's own runtime helpers and the `kelp_rt.c` entry points read
    /// the same fields by the same offsets. The verifier cannot see the object
    /// and a Kelp-only fixture cannot see a reordered field that both sides of
    /// an IR-to-IR call agree on, so the agreement is pinned where it is made.
    ///
    /// The list is the whole container surface: `List` and `Bytes`/`String`
    /// share the `{ptr, len, cap}` shape, `Box` is one pointer, the map is the
    /// dense entries plus the separate index, one entry is `{key, value*, live}`,
    /// and an array is its inline storage. A change to any of these lines is a
    /// change to the ABI the C runtime and the monomorphized sets share.
    #[test]
    fn the_container_layouts_match_the_c_typedefs() {
        let ctx = Context::new();
        let module = Module::new(&ctx, "containers");
        let ptr = ctx.opaque_ptr();
        let i64t = ctx.i64();
        let i8t = ctx.i8();
        let string = ctx.struct_type("kelp_String");
        ctx.struct_set_body(string, &[ptr, i64t, i64t]);
        let list = ctx.struct_type("kelp_List__i32");
        ctx.struct_set_body(list, &[ptr, i64t, i64t]);
        let boxed = ctx.struct_type("kelp_Box__i32");
        ctx.struct_set_body(boxed, &[ptr]);
        let entry = ctx.struct_type("kelp_Map__String__i32_entry");
        ctx.struct_set_body(entry, &[string, ptr, i8t]);
        let map = ctx.struct_type("kelp_Map__String__i32");
        ctx.struct_set_body(map, &[ptr, i64t, i64t, i64t, ptr, i64t, i64t]);
        let arr = ctx.struct_type("kelp_Array__u8__4");
        ctx.struct_set_body(arr, &[ctx.array_type(ctx.i8(), 4)]);
        // A named type is only printed once something references it.
        module.add_function("probe_list", ctx.fn_type(list, &[], false));
        module.add_function("probe_box", ctx.fn_type(boxed, &[], false));
        module.add_function("probe_map", ctx.fn_type(map, &[], false));
        module.add_function("probe_entry", ctx.fn_type(entry, &[ptr], false));
        module.add_function("probe_array", ctx.fn_type(arr, &[], false));
        let ir = module.to_ir_string();
        for want in [
            "%kelp_List__i32 = type { ptr, i64, i64 }",
            "%kelp_Box__i32 = type { ptr }",
            "%kelp_Map__String__i32_entry = type { %kelp_String, ptr, i8 }",
            "%kelp_Map__String__i32 = type { ptr, i64, i64, i64, ptr, i64, i64 }",
            "%kelp_Array__u8__4 = type { [4 x i8] }",
        ] {
            assert!(ir.contains(want), "missing '{want}':\n{ir}");
        }
    }

    /// The C-boundary rule (BUG-vN4tR, MK7 3.3): an aggregate over two
    /// eightbytes crosses a C boundary only through a pointer. The threshold is
    /// the target's own data layout, so exactly sixteen bytes is not over it and
    /// a three-field `datac` is. `declare_extern` and `rt_arg_value` both ask
    /// this one function, so this is the rule the two sites share.
    #[test]
    fn an_aggregate_over_two_eightbytes_crosses_a_c_boundary_by_pointer() {
        use super::crosses_c_boundary_by_value;
        use crate::llvm_ffi::{initialize_native_target, TargetMachine};

        initialize_native_target().expect("the native target");
        let machine = TargetMachine::for_host().expect("a target machine for the host");
        let layout = machine.data_layout();
        let ctx = Context::new();
        let view = ctx.struct_type("kelp_View_crossing_test");
        ctx.struct_set_body(view, &[ctx.opaque_ptr(), ctx.i64()]);
        let pair = ctx.struct_type("kelp_Pair_crossing_test");
        ctx.struct_set_body(pair, &[ctx.i64(), ctx.i64()]);
        let wide = ctx.struct_type("kelp_Wide_crossing_test");
        ctx.struct_set_body(wide, &[ctx.i64(), ctx.i64(), ctx.i64()]);

        assert!(!crosses_c_boundary_by_value(&Ty::View, view, &layout));
        assert!(!crosses_c_boundary_by_value(
            &Ty::Struct("Pair".to_string()),
            pair,
            &layout
        ));
        assert!(crosses_c_boundary_by_value(
            &Ty::Struct("Wide".to_string()),
            wide,
            &layout
        ));
    }
}

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
//! A move is a load and a store (MK7 §3.4). Every struct this slice lowers has a
//! field set that owns nothing, so `types::needs_drop` is false for it and the
//! drop plan is empty: there is no second drop to leave behind because there is
//! no first one. An owned field is the container/drop slice and is refused by
//! name below.
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
use crate::llvm_ffi::{
    self, BasicBlock, Builder, Context, FloatBinOp, FnType, Function as LlvmFunction, IntBinOp,
    IntCmp, Module, RealCmp, TargetMachine, Type, Value,
};
use crate::tir::{Block, Expr, ExprKind, Function, Program, Stmt, TemplatePiece};
use crate::types::{needs_drop, owned_types, struct_tag, Ty};

/// Build the whole program, verify it, write the textual IR beside the binary,
/// emit the object and link it with `cc`.
///
/// The artifact contract mirrors the C path's `<out>.c`: this writes `<out>.ll`
/// (MK7 §4.4) so the IR is inspectable next to the binary, and keeps the object
/// it hands `cc` as `<out>.o`. Linking itself is `driver::link_llvm_object`, the
/// sibling of `driver::compile_c`.
pub fn compile(program: &Program, out_path: &Path) -> Result<(), String> {
    llvm_ffi::initialize_native_target()?;
    let triple = llvm_ffi::default_target_triple();
    if triple.is_empty() {
        return Err("LLVM returned an empty host triple".to_string());
    }

    let ctx = Context::new();
    let mut module = Module::new(&ctx, "kelp");
    module.set_triple(&triple);

    let mut lowering = Gen::new(&ctx, &module, program)?;
    lowering.declare()?;
    lowering.define_panic();
    lowering.emit_all()?;

    // A caller must verify before emitting: LLVM's object writer assumes a
    // verified module and can crash on an unverified one rather than fail.
    module.verify()?;

    let ir_path = out_path.with_extension("ll");
    std::fs::write(&ir_path, module.to_ir_string()).map_err(|e| {
        format!("could not write generated IR to '{}': {e}", ir_path.display())
    })?;

    let object_path = out_path.with_extension("o");
    let machine = TargetMachine::for_host()?;
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
    fprintf_ty: FnType<'a>,
    exit_ty: FnType<'a>,
    panic_fn: LlvmFunction<'a>,
    panic_ty: FnType<'a>,
    stderr: Value<'a>,
    /// One identified LLVM struct type per `datac`, keyed by the `tir` name. The
    /// body is set once per declaration in `declare_structs`, before any
    /// function signature is built, so a `datac` parameter or return names a
    /// complete type.
    structs: HashMap<String, Type<'a>>,
    /// Each `datac`'s field names in declaration order, so a `FieldAccess`
    /// resolves its field to the struct index a `getelementptr` needs without
    /// re-walking `program.structs`.
    field_index: HashMap<String, Vec<String>>,
}

impl<'a> Gen<'a> {
    fn new(ctx: &'a Context, module: &'a Module<'a>, program: &'a Program) -> Result<Self, String> {
        let ptr = ctx.opaque_ptr();
        // The runtime symbols S1 reaches: libc's `printf`/`fprintf`/`exit` and
        // `stderr`, plus the IR-defined `kelp_panic` below. Declared once, with
        // the one type each is called with.
        let printf_ty = ctx.fn_type(ctx.i32(), &[ptr], true);
        let printf_fn = module.add_function("printf", printf_ty);
        let fprintf_ty = ctx.fn_type(ctx.i32(), &[ptr, ptr], true);
        module.add_function("fprintf", fprintf_ty);
        let exit_ty = ctx.fn_type(ctx.void(), &[ctx.i32()], false);
        module.add_function("exit", exit_ty);
        let panic_ty = ctx.fn_type(ctx.void(), &[ptr], false);
        let panic_fn = module.add_function("kelp_panic", panic_ty);
        let stderr = module.add_external_global("stderr", ptr);
        Ok(Gen {
            ctx,
            module,
            builder: Builder::new(ctx),
            program,
            funcs: HashMap::new(),
            printf_fn,
            printf_ty,
            fprintf_ty,
            exit_ty,
            panic_fn,
            panic_ty,
            stderr,
            structs: HashMap::new(),
            field_index: HashMap::new(),
        })
    }

    /// Refuse the program-level constructs this slice does not lower, declare
    /// every `datac`, then declare every function and the runtime symbols.
    ///
    /// Refusing before any function is emitted means a program with an `enum`
    /// anywhere fails with the construct named, rather than with the first
    /// expression that mentions it.
    fn declare(&mut self) -> Result<(), String> {
        let program = self.program;
        // The `datac` types first, so a field this slice cannot lay out is
        // reported by its own construct rather than behind an unrelated std
        // declaration.
        self.declare_structs()?;
        // A `pd` is a module global plus an `atexit` registration, which is the
        // runtime slice's job.
        if !program.pds.is_empty() {
            return Err(refuse("'pd'", "S3"));
        }
        // An `enum` is the tagged-union slice (the `{tag, union}` layout and the
        // `switch` per drop, MK7 §3.1), separate from the plain-record lowering
        // this slice does, so it keeps its own stage label.
        if !program.enums.is_empty() {
            return Err(refuse("'enum'", "S2C"));
        }
        // `program.caps` holds each module's granted patterns, which is data, not
        // a construct: it is harmless to ignore while nothing calls a gated
        // function. A gated *call* is a stdlib `extern fun` and is refused by
        // name at the call site (S3).

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
                return Err(refuse("'a closure'", "S2B"));
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
    fn declare_structs(&mut self) -> Result<(), String> {
        let program = self.program;
        let ctx = self.ctx;
        let owned = owned_types(&program.structs, &program.enums);
        let enum_names: HashSet<&str> = program.enums.iter().map(|e| e.name.as_str()).collect();
        for s in &program.structs {
            for f in &s.fields {
                refuse_datac_field(&f.ty, &owned, &enum_names)?;
            }
        }
        for s in &program.structs {
            let ty = ctx.struct_type(&struct_tag(&s.name));
            self.structs.insert(s.name.clone(), ty);
            self.field_index.insert(
                s.name.clone(),
                s.fields.iter().map(|f| f.name.clone()).collect(),
            );
        }
        for s in &program.structs {
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
        Ok(())
    }

    /// Define `void @kelp_panic(ptr)`: the runtime's panic, written in IR here
    /// because S1 links no runtime object (the C `librt` is S3, MK7 §3.2).
    ///
    /// It prints the same `panic: <msg>` line the C runtime's `kelp_panic` does
    /// and exits 1, so the panic text a conversion's range check names is
    /// byte-identical to the C backend's.
    fn define_panic(&self) {
        let f = self.panic_fn;
        let entry = f.append_block("entry");
        self.builder.position_at_end(entry);
        let stderr = self
            .builder
            .build_load(self.ctx.opaque_ptr(), self.stderr, "stderr");
        let fmt = self.builder.global_string("panic: %s\n", "panic.fmt");
        let msg = f.get_param(0);
        let fprintf = self
            .module
            .get_function("fprintf")
            .expect("fprintf was declared in declare()");
        self.builder.build_call(fprintf, self.fprintf_ty, &[stderr, fmt, msg], "");
        let exit = self
            .module
            .get_function("exit")
            .expect("exit was declared in declare()");
        self.builder
            .build_call(exit, self.exit_ty, &[self.ctx.const_int(self.ctx.i32(), 1, false)], "");
        self.builder.build_ret_void();
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
    /// belongs to a value-returning function, discarded otherwise (the C backend
    /// draws the same line in `write_function_body`).
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
        if let Some(tail) = &block.tail {
            if self.ret_ty != Ty::Unit {
                let value = self.value_of(tail)?;
                self.g.builder.build_ret(value);
                self.terminated = true;
            } else {
                self.emit_expr(tail)?;
            }
        }
        Ok(())
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
        match &block.tail {
            Some(tail) => self.emit_expr(tail),
            None => Ok(None),
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
            Stmt::Return { value, .. } => match value {
                Some(e) => {
                    let v = self.value_of(e)?;
                    self.g.builder.build_ret(v);
                    self.terminated = true;
                }
                None => self.emit_fallthrough_return()?,
            },
            Stmt::While { cond, body } => self.emit_while(cond, body)?,
            Stmt::Expr(e) => {
                self.emit_expr(e)?;
            }
        }
        Ok(())
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
            ExprKind::Ident(name) => Ok(Some(self.load_ident(name, &e.ty)?)),
            ExprKind::Unary { op, expr } => Ok(Some(self.emit_unary(*op, expr, &e.ty)?)),
            ExprKind::Binary { op, lhs, rhs } => {
                Ok(Some(self.emit_binary(*op, lhs, rhs)?))
            }
            ExprKind::Call { callee, args } => self.emit_call(callee, args),
            ExprKind::Assign { target, value } => {
                self.emit_assign(target, value)?;
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
            // A borrow (`infer.rs::check_call_arg` synthesizes one for the
            // `datac` parameter whose type resolves to a shared or exclusive
            // borrow). `&str` is the pointer itself, as the C backend writes it;
            // a `datac` borrow is the address of the place it borrows, which is
            // the one indirection `getelementptr` needs.
            ExprKind::Ref { expr: sub, .. } => {
                if is_str_ref(&e.ty) {
                    return Ok(Some(self.value_of(sub)?));
                }
                if is_struct_ref(&e.ty) {
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
    /// place, or the pointer a `datac` borrow names.
    ///
    /// An rvalue base (`mk().field`, a literal's field) has no address, so it is
    /// stored into a temporary slot first, exactly as `codegen_c.rs` hoists one.
    /// The temporary owns nothing here: every struct this slice lowers is
    /// dropless, so there is nothing to free and no drop plan to run.
    fn place_of(&mut self, e: &Expr) -> Result<Value<'a>, String> {
        match &e.kind {
            ExprKind::Ident(name) => {
                let slot = self
                    .lookup(name)
                    .ok_or_else(|| no_value(&format!("the local '{name}'")))?;
                if is_struct_ref(&e.ty) {
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
            // A borrow in a place position is already an address.
            ExprKind::Ref { expr: sub, .. } => self.place_of(sub),
            ExprKind::Deref { expr: sub } => self.value_of(sub),
            _ => {
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

    fn load_ident(&self, name: &str, ty: &Ty) -> Result<Value<'a>, String> {
        let slot = self
            .lookup(name)
            .ok_or_else(|| no_value(&format!("the local '{name}'")))?;
        let lty = self.lty(ty)?;
        Ok(self.g.builder.build_load(lty, slot, name))
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

    fn emit_assign(&mut self, target: &Expr, value: &Expr) -> Result<(), String> {
        let v = self.value_of(value)?;
        if self.terminated {
            return Ok(());
        }
        match &target.kind {
            ExprKind::Ident(name) => {
                let slot = self
                    .lookup(name)
                    .ok_or_else(|| no_value(&format!("the local '{name}'")))?;
                self.g.builder.build_store(v, slot);
                Ok(())
            }
            // A field write: the field's address, then a store of the new value.
            // A struct this slice lowers owns nothing, so the value being
            // overwritten needs no drop first; the drop plan (MK7 §3.4) belongs
            // to the owned-field slice and is not reached here.
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
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
            for a in args {
                values.push(self.value_of(a)?);
            }
            let v = self.g.builder.build_call(info.func, info.ty, &values, "call");
            if info.ret_ty == Ty::Unit {
                return Ok(None);
            }
            return Ok(Some(v));
        }

        // Anything else is outside this slice. An `extern fun` is the runtime
        // and is S3; any other name is a construct this stage has no lowering
        // for.
        let stage = if self.g.program.functions.iter().any(|f| f.name == callee && f.extern_) {
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
    /// `codegen_c.rs::printf_piece_with` uses, so stdout is byte-identical.
    fn emit_template_print(&mut self, newline: bool, arg: &Expr) -> Result<(), String> {
        let (spec, value) = self.printf_piece(arg)?;
        let fmt = if newline {
            format!("{spec}\n")
        } else {
            spec.to_string()
        };
        let fmt_ptr = self.string_global(&fmt)?;
        self.g
            .builder
            .build_call(self.g.printf_fn, self.g.printf_ty, &[fmt_ptr, value], "");
        Ok(())
    }

    /// A template: one `printf` whose format string is the literal chunks (with
    /// `%` doubled) plus one specifier per interpolated value. The C backend
    /// builds the same format string in `codegen_c.rs`'s `Template` arm.
    fn emit_template(
        &mut self,
        newline: bool,
        pieces: &[TemplatePiece],
    ) -> Result<(), String> {
        let mut fmt = String::new();
        let mut args: Vec<Value<'a>> = Vec::new();
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
                    let (spec, v) = self.printf_piece(value)?;
                    fmt.push_str(spec);
                    args.push(v);
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
        Ok(())
    }

    /// The format specifier and the already-promoted argument for one value,
    /// mirroring `codegen_c.rs::printf_piece_with` exactly.
    fn printf_piece(&mut self, arg: &Expr) -> Result<(&'static str, Value<'a>), String> {
        let v = self.value_of(arg)?;
        Ok(match &arg.ty {
            Ty::I32 => ("%d", v),
            Ty::I64 => ("%lld", v),
            Ty::U32 => ("%u", v),
            Ty::U64 => ("%llu", v),
            // A vararg narrower than `int` must be promoted here: LLVM does not,
            // unlike C, whose default argument promotions do it for free.
            Ty::U8 | Ty::U16 => (
                "%u",
                self.g
                    .builder
                    .build_int_cast(false, true, v, self.g.ctx.i32(), "promote"),
            ),
            Ty::Usize => ("%zu", v),
            Ty::Isize => ("%lld", v),
            // `%f` reads a `double` out of the vector file, so an `f32` is
            // promoted, exactly as C's default argument promotions do.
            Ty::F32 => (
                "%f",
                self.g
                    .builder
                    .build_fp_cast(true, v, self.g.ctx.f64(), "promote"),
            ),
            Ty::F64 => ("%f", v),
            Ty::Bool => {
                let t = self.string_global("true")?;
                let f = self.string_global("false")?;
                ("%s", self.g.builder.build_select(v, t, f, "bool"))
            }
            Ty::Char => ("%c", v),
            Ty::Str => ("%s", v),
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => ("%s", v),
            other => return Err(refuse_type(other)),
        })
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

/// Map a `tir` type to its LLVM type. The scalar subset, `str`, and a `datac`
/// `structs` holds a type for.
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
/// when it is a shape; `stage` is the MK7 stage that will lower it: this slice is
/// S2A (the plain-record lowering), S2B is the container and drop slice, S2C is
/// the tagged union and the borrowed views, and S3 is the runtime.
fn refuse(what: &str, stage: &str) -> String {
    format!("the llvm backend cannot lower {what} yet (MK7 {stage}); use --backend=c")
}

/// Refuse a `datac` field this slice cannot lay out, with the construct named.
///
/// An owned field is the drop plan's problem (`types::needs_drop` is exactly
/// "this field owns or transitively owns heap memory"), so it gets the one
/// S2B wording rather than the field type's own; everything else that is not a
/// scalar or an all-scalar `datac` is named by [`refuse_type`]. A `str`/`View`
/// field is unreachable from the frontend (`types::is_borrowed_field_ty`
/// rejects it), but it is refused here rather than mis-lowered if it ever
/// appears. A field that names an `enum` is refused as the tagged-union slice:
/// an `enum` is a `Ty::Struct` at the type level, so without this the enum name
/// would be reported as a missing `datac`.
fn refuse_datac_field(
    ty: &Ty,
    owned: &HashSet<String>,
    enum_names: &HashSet<&str>,
) -> Result<(), String> {
    if needs_drop(ty, owned) {
        return Err(refuse(
            &format!("'a datac field of an owned type ({})'", ty.display()),
            "S2B",
        ));
    }
    match ty {
        Ty::Str => Err(refuse("'a 'str' field'", "S2C")),
        Ty::View => Err(refuse("'a 'View' field'", "S2C")),
        Ty::Ref { .. } => Err(refuse("'a reference field'", "S2B")),
        Ty::Fn { .. } | Ty::Closure { .. } => Err(refuse("'a function-valued field'", "S2B")),
        Ty::Struct(name) if enum_names.contains(name.as_str()) => {
            Err(refuse("'an enum field'", "S2C"))
        }
        Ty::Struct(_) => Ok(()),
        other if llvm_scalar_type(other) => Ok(()),
        other => Err(refuse_type(other)),
    }
}

/// Whether a `tir` type is one of the scalars the S1 lowering already emits. A
/// type that is not a scalar and not a `datac` is refused by name in
/// `refuse_datac_field`, so this only has to recognize the scalars.
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

/// Name the type in a refusal, with the stage that will lower it. Anything that
/// owns memory is the container and drop slice (S2B); a borrowed view is S2C.
fn refuse_type(ty: &Ty) -> String {
    let (what, stage) = match ty {
        Ty::Struct(_) => ("'datac'", "S2A"),
        Ty::Bytes => ("'Bytes'", "S2B"),
        Ty::String => ("'String'", "S2B"),
        Ty::List(_) => ("'List'", "S2B"),
        Ty::Box(_) => ("'Box'", "S2B"),
        Ty::Map(..) => ("'Map'", "S2B"),
        Ty::Array(..) => ("'an array'", "S2B"),
        Ty::Ref { .. } => ("'a borrow of a non-datac'", "S2B"),
        Ty::Fn { .. } => ("'a function value'", "S2B"),
        Ty::Closure { .. } => ("'a closure'", "S2B"),
        Ty::Str | Ty::View => ("a borrowed view", "S2C"),
        Ty::Unit => ("'unit'", "S1"),
        other => return refuse(&format!("the type '{}'", other.display()), "S1"),
    };
    refuse(what, stage)
}

/// Name the expression in a refusal. A `FieldAccess` or a `StructLit` never
/// reaches here: both are lowered. An `enum` construct is the tagged-union slice
/// (S2C), and a function value or a container literal is S2B.
fn refuse_expr(kind: &ExprKind) -> String {
    let (what, stage) = match kind {
        ExprKind::EnumLit { .. } | ExprKind::EnumTag { .. } | ExprKind::EnumField { .. } => {
            ("'an enum'", "S2C")
        }
        ExprKind::Try { .. } => ("'the ? operator'", "S2C"),
        ExprKind::FnRef(_) | ExprKind::FnToClosure { .. } => ("'a function value'", "S2B"),
        ExprKind::ClosureLit { .. } => ("'a closure'", "S2B"),
        ExprKind::Capture { .. } => ("'a capture'", "S2B"),
        ExprKind::IndirectCall { .. } => ("'a call through a function value'", "S2B"),
        ExprKind::Ref { .. } => ("'a borrow of a non-datac'", "S2B"),
        ExprKind::Deref { .. } => ("'a dereference of a non-datac'", "S2B"),
        ExprKind::ArrayLit(_) | ExprKind::ArrayRepeat { .. } => ("'an array'", "S2B"),
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

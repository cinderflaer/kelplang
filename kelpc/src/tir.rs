//! Typed intermediate representation produced by `infer.rs` and consumed by
//! `codegen_c.rs` (and, from M3 onward, `borrowck.rs`).

use crate::ast::{AsmOperandKind, BinOp, UnOp};
use crate::lexer::Span;
use crate::types::{EnumDef, StructDef, Ty};

#[derive(Debug, Clone)]
pub struct Program {
    pub structs: Vec<StructDef>,
    /// `enum` declarations (A-1), in the same merged declaration order as
    /// `structs` - each carries its `order`, so `codegen_c.rs` can emit the
    /// two kinds in one interleaved sequence. An `enum` value is a
    /// `Ty::Struct` at the type level; this table is what makes it a tagged
    /// union instead of a record.
    pub enums: Vec<EnumDef>,
    /// Passdowns (M5.2) in initialization order - module load order, then
    /// declaration order within a module. `codegen_c.rs` emits one global
    /// per entry plus a generated init function that runs before `main`'s
    /// body.
    pub pds: Vec<Pd>,
    pub functions: Vec<Function>,
    /// Capability grants, one per (module, domain) that holds any (M7).
    /// `codegen_c.rs` emits each as constant data and checks the arguments of
    /// every gated call against it.
    pub caps: Vec<CapsGrant>,
    /// Function key -> the capability domain of the module that declares it.
    /// Only gated modules appear, so this is also the set of calls that need
    /// a runtime allowlist check.
    pub gated: std::collections::HashMap<String, String>,
    /// The user types that declare their own `Map` key pair (A-12): a `datac`
    /// carrying both `fun T.hash().return: u64` and `fun T.eq(other: &T).return: bool`.
    /// `infer.rs` is the only place that decides it, so the two backends
    /// cannot disagree about which key spells its own hash: each reads this
    /// set to point the `Map`'s hash and equality at those two methods instead
    /// of the derived `kelp_hash_<T>`/`kelp_eq_<T>` pair it does not emit for
    /// them.
    pub declared_keys: std::collections::HashSet<String>,
    /// The user types that declare their own destructor (kernel stage 9,
    /// MK9-S6-M1): a `datac`/`enum` with `fun mut T.drop()`. `infer.rs` is the
    /// only place that decides it, so both backends and the drop-plan seeding
    /// read the same set: `types::owned_types` seeds its owned set with these,
    /// which is what gives a destructor type a `kelp_drop_<T>` even when every
    /// field is a scalar, and each backend calls the user body at the top of
    /// that function.
    pub drop_types: std::collections::HashSet<String>,
    /// Full (prefixed) function name -> the bare native symbol it binds to,
    /// for a *user*-declared `extern fun` (MK9-S1-M1, roadmap B-1) -
    /// `codegen_c.rs::write_extern` reads this instead of `externs.rs` when a
    /// function's module has no compiler-supplied body, and writes a
    /// direct-call trampoline to the symbol rather than looking up compiled-in
    /// C text. Empty for a program that declares no user FFI, which behaves
    /// exactly as before this feature existed.
    pub extern_ffi_symbols: std::collections::HashMap<String, String>,
    /// The C headers the project's `[extern.*] header` keys name,
    /// deduplicated and sorted. `codegen_c.rs` emits one `#include <header>`
    /// line per entry after the fixed preamble, and `codegen_llvm.rs` writes
    /// the same lines into the separately-compiled extern object, so a native
    /// symbol the runtime does not already declare has a prototype. Empty
    /// when no section names a header, which leaves the generated C text
    /// byte-for-byte what it was before the key existed.
    pub extern_headers: Vec<String>,
    /// The canonical names of the program's `extern type` handles (gap 1),
    /// sorted so both backends emit the same order. Each one is `void*`
    /// behind its own C typedef, emitted with the other type definitions; a
    /// program that declares none leaves the generated output unchanged.
    pub handles: Vec<String>,
    /// The top-level integer-array `const`s that carry a `section("...")`
    /// clause (kernel stage 4), sorted by name so both backends emit them in
    /// one order. Each is emitted as read-only data in its section so a linker
    /// script can place it; uses of the const still fold to the literal, so the
    /// global is a symbol for the linker, not a load. Empty for a program that
    /// writes no such clause, which leaves both backends' output unchanged.
    pub const_data: Vec<ConstData>,
    /// The program's `static` globals (kernel stage 7), in declaration order.
    /// Each is one object with a stable address; `infer.rs` has already folded
    /// its initializer into a constant tree, so no code runs at startup.
    pub statics: Vec<Static>,
    /// One vtable per `(Trait, Type)` pair a coercion used (A-12 slice 4),
    /// sorted by `(trait, type)` so both backends emit the same order. Empty
    /// for a program that writes no `&dyn`, which leaves both backends' output
    /// unchanged.
    pub dyn_vtables: Vec<DynVtable>,
    /// Every trait a `dyn` type names, borrowed or owned (A-12 slices 4 and
    /// 7), sorted. The fat-pair typedef and the vtable struct tag are emitted
    /// per *trait*, so a `dyn` type that is written but never coerced to still
    /// has a C type to name. Empty for a program with no trait object.
    pub dyn_traits: Vec<String>,
}

/// One `(Trait, Type)` vtable (A-12 slice 4): the static, read-only table a
/// `&dyn Trait` value points at. `slots` is the trait's flattened method list
/// (`plan/traits.md` 9.4): the supertraits' methods first, recursively in
/// declaration order, then the trait's own methods in source order. Every
/// vtable of one trait has the same slots in the same order, so a call site
/// names a slot index and the backend reads it out of whichever vtable the
/// value carries.
#[derive(Debug, Clone)]
pub struct DynVtable {
    /// The trait's canonical key.
    pub trait_name: String,
    /// The implementing type's name as an `impl` wrote it (a canonical
    /// `datac`/`enum` key, or a builtin's written name such as `i32`).
    pub type_name: String,
    /// The concrete type itself, recovered from the concrete method's own
    /// receiver. The drop entry needs it: dropping a `Box<dyn Trait>` runs this
    /// type's drop glue through the vtable, and the name alone cannot say which
    /// `Ty` it is (a `datac` key, a scalar, `String`, an atomic alias).
    pub concrete: Ty,
    pub slots: Vec<DynVtableSlot>,
}

/// One method of one vtable: the concrete function the thunk calls and the
/// signature the thunk (and therefore the slot) has.
#[derive(Debug, Clone)]
pub struct DynVtableSlot {
    /// The trait method's own name, which is also the vtable field's name in
    /// the C backend.
    pub method: String,
    /// The concrete method's key, e.g. `shapes.Circle#area`.
    pub target: String,
    /// The concrete method's parameter types, receiver first. `params[0]` is
    /// the receiver: a `Ty::Ref` means the erased data pointer *is* the
    /// receiver, anything else a `Copy` builtin the thunk loads from it.
    pub params: Vec<Ty>,
    pub ret: Ty,
}

/// One `static NAME: T = INIT` global (kernel stage 7).
#[derive(Debug, Clone)]
pub struct Static {
    /// The module-prefixed top-level name, which is the emitted symbol (each
    /// backend mangles it with its own prefix).
    pub name: String,
    /// Canonical id of the module this `static` was declared in, like
    /// `Function::module`: the split backend needs it to know which unit owns
    /// the global's definition (roadmap A-14), since the name alone is
    /// prefix-relative and the entry module's prefix is empty.
    pub module: String,
    pub ty: Ty,
    /// `static mut NAME` - every access is `unsafe`.
    pub mutable: bool,
    /// `percpu static mut NAME` (kernel stage 8): the static is one object per
    /// CPU. The backends emit it as the manifest's `[target] cpus` copies in
    /// `.percpu`, each 64-byte aligned, and a reference to the name indexes the
    /// copy for the current CPU.
    pub percpu: bool,
    /// The `section("...")` clause, when one was written.
    pub section: Option<String>,
    pub init: StaticInit,
}

/// A `static`'s folded initializer (kernel stage 7). A small tree rather than a
/// `tir::Expr`, because both backends need a *constant* of the global's type:
/// the C backend writes a compound initializer and the LLVM backend builds an
/// `LLVMConst*`, and neither can run code at startup (a freestanding image has
/// no init hook).
#[derive(Debug, Clone, PartialEq)]
pub enum StaticInit {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(u32),
    /// `[a, b, c]` - one entry per element, in order.
    Array(Vec<StaticInit>),
    /// `[v; N]` - the same value repeated. Kept apart from `Array` so a 4096-byte
    /// zero buffer stays one `zeroinitializer` (or one `= { 0 }`), not 4096
    /// entries the code generators would each have to spell.
    Repeat { value: Box<StaticInit>, len: u64 },
    /// A `datac` constructor's fields, in declaration order.
    Struct(Vec<StaticInit>),
    /// `Atomic.new(v)` - the cell's one member.
    Atomic(Box<StaticInit>),
}

/// One `section(".rodata") const NAME: [u8; N] = [...]` (kernel stage 4).
#[derive(Debug, Clone)]
pub struct ConstData {
    /// The module-prefixed top-level name, which is the emitted symbol
    /// (`kelp_...`/the C backend's own mangling is applied by each backend).
    pub name: String,
    /// Canonical id of the module this `const` was declared in; see
    /// `Static::module`.
    pub module: String,
    /// The array's element type and length, already checked.
    pub ty: Ty,
    /// The section the `section(...)` clause named.
    pub section: String,
    /// One value per element, in order, each already cut to the element type.
    pub values: Vec<i128>,
}

/// One module's patterns for one capability domain.
#[derive(Debug, Clone)]
pub struct CapsGrant {
    pub module: String,
    pub domain: String,
    pub patterns: Vec<String>,
}

/// One checked passdown. `name` is the internal `pd#<qualified>` name shared
/// by the generated C global and the hidden parameter every referencing
/// function receives (see `infer::PdInfo`).
#[derive(Debug, Clone)]
pub struct Pd {
    pub name: String,
    /// Module-prefixed name, for diagnostics and comments in the C output.
    pub qualified: String,
    pub value_ty: Ty,
    pub init: Expr,
    /// Canonical id of the module this `pd` was declared in, like
    /// `Function::module`: an initializer is not a function, so this is the
    /// only carrier of the module whose grant a gated call inside it is
    /// checked against, and of the module the denial names.
    pub module: String,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    pub body: Block,
    /// Source span of the declaration (or, for a `fun` literal, of the literal
    /// itself). Like every other `Span` here it is only meaningful together
    /// with `module`, and it is what a backend names as the function's own
    /// line: the C backend's diagnostics and the LLVM backend's `DISubprogram`
    /// both resolve it through `diag::SourceMap`.
    pub span: Span,
    /// Canonical id of the module this function was declared in - needed so
    /// `borrowck.rs` can tag its diagnostics with the right source file (a
    /// `Span` is only meaningful inside its own file).
    pub module: String,
    /// `extern fun` (M6.2): the body below is synthetic (empty) and
    /// `codegen_c.rs` writes the compiler-supplied C for it instead.
    pub extern_: bool,
    /// The environment this function receives, when it is a `fun` literal that
    /// captured something (A-4 phase E): `Some` makes the generated C
    /// signature start with a hidden `void*` environment parameter that the
    /// body's `ExprKind::Capture` reads go through. `None` for every ordinary
    /// function and for a literal that captured nothing.
    pub env: Option<ClosureEnv>,
    /// `naked fun ...` (kernel stage 5): LLVM's `naked` and `noinline`. The
    /// body is exactly one `asm(...)` statement, so no prologue or epilogue is
    /// emitted around it.
    pub naked: bool,
    /// The `align(N)` clause a declaration carried (kernel gap 2), or `None`
    /// for the target's default function alignment. Each backend sets the
    /// function's alignment to this value.
    pub align: Option<u32>,
    /// The `section("...")` clause a declaration carried (kernel stage 4), or
    /// `None` for the target's default `.text`. Each backend places the
    /// function's code in this section.
    pub section: Option<String>,
}

/// One literal's environment (A-4 phase E); see `Function::env`.
#[derive(Debug, Clone)]
pub struct ClosureEnv {
    /// The generated C struct (`kelp_Env__#funlit3`), one field per capture.
    pub struct_name: String,
    /// The generated function that drops each owned field and frees the
    /// struct (`kelp_env_drop__#funlit3`).
    pub drop_name: String,
    /// Field name (`c0`, `c1`, …) and type, in the order `ExprKind::Capture`
    /// indexes them.
    pub fields: Vec<(String, Ty)>,
}

/// The place a `for` walks, for the borrow checker's mutation refusal
/// (`plan/syntax.md` 6.2): the path is the iterable as written (`xs`,
/// `self.items`) and the span is that place on the `for` line.
#[derive(Debug, Clone)]
pub struct IteratedPlace {
    pub path: String,
    pub span: Span,
}

/// The canonical path of a place expression, the same rendering `ast::place_path`
/// gives the surface expression, so a mutation target `borrowck.rs` reads out of
/// the typed tree compares equal to the path `infer.rs` recorded.
pub fn place_path(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::FieldAccess { base, field } => {
            place_path(base).map(|b| format!("{b}.{field}"))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// The block's value: the last statement's expression, if it was written
    /// without a trailing semicolon. `None` means the block's value is unit.
    pub tail: Option<Box<Expr>>,
}

impl Block {
    pub fn ty(&self) -> Ty {
        self.tail.as_ref().map(|e| e.ty.clone()).unwrap_or(Ty::Unit)
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        ty: Ty,
        init: Expr,
    },
    While {
        cond: Expr,
        body: Block,
        /// The place a `for` loop walks, when it walks one (`xs`, `self.items`):
        /// the canonical path a body mutation is compared against, and the
        /// `for` line's span a refusal names. `None` for a written `while` and
        /// for a `for` over a temporary (`plan/syntax.md` 6.2).
        iterating: Option<IteratedPlace>,
    },
    Return {
        value: Option<Expr>,
        /// Source span of the `return` statement itself, for the same reason
        /// `Block::span` exists: dropping the frame's live owned locals
        /// before returning is keyed by it.
        span: Span,
    },
    /// `break` (`plan/iterators.md`): leave the innermost loop. The span keys
    /// the drops of the locals declared inside that loop (`DropPlan::at_break`),
    /// the same way `Return::span` keys the frame's.
    Break { span: Span },
    /// `continue`: skip to the next pass of the innermost loop, running the
    /// drops of the locals declared inside it first.
    Continue { span: Span },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    /// Source span of the surface-syntax expression this was checked from -
    /// needed for borrowck's "value moved here" / "used here" diagnostics.
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    /// An integer literal, at `i128` so that a `const` folded to the top of
    /// `u64`'s range is representable; codegen cuts it to the expression's own
    /// type, which is what makes the width the type's rather than the literal's.
    IntLit(i128),
    /// `'a'` - the code point, already checked against the type
    /// `infer.rs` resolved for it. Codegen casts it to that type's C
    /// spelling, so a `'0'` in a `u8` position emits a `uint8_t`.
    CharLit(u32),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),
    Ident(String),
    /// A read of a top-level `static` (kernel stage 7). A *place*, not a value:
    /// codegen loads from it in value position, stores through it for an
    /// assignment, and takes its address for `Ptr.from_ref`/`Ptr.from_mut`.
    /// `name` is the module-prefixed symbol.
    StaticRef(String),
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: String,
        args: Vec<Expr>,
    },
    /// A call to a *user* `extern fun` whose last parameter is `...` (gap 7).
    /// `callee` is still the extern's own key, because everything the call
    /// needs from it (the capability domain, the diagnostic name) is keyed by
    /// that name; `args` holds the fixed arguments and then the extra ones.
    /// `wrapper` is the key of the generated fixed-arity C function the call
    /// actually makes, one per distinct list of extra-argument types - the
    /// typechecker picks it, so neither backend ever emits a variadic call
    /// itself and the C compiler's default promotions do the rest.
    VariadicCall {
        callee: String,
        wrapper: String,
        args: Vec<Expr>,
    },
    FieldAccess {
        base: Box<Expr>,
        field: String,
    },
    StructLit {
        name: String,
        fields: Vec<(String, Expr)>,
        borrowed_fields: Vec<String>,
    },
    /// Construction of one `enum` variant (A-1): `Shape.Circle(r = 1.0)`.
    /// `variant` is the variant's index within the enum, and `fields` is one
    /// value per payload field in declaration order. Emitted as a C compound
    /// literal whose `tag` is that index.
    EnumLit {
        enum_name: String,
        variant: usize,
        fields: Vec<Expr>,
    },
    /// Whether the scrutinee's live tag is `variant` - the test that a
    /// `when` arm lowers to. `ty` is `bool`.
    EnumTag {
        expr: Box<Expr>,
        variant: usize,
    },
    /// One payload field of one variant, read out of a value whose tag is
    /// known to be `variant`. A *place* (like `FieldAccess`), which is what
    /// lets an owned payload be bound as a shared borrow rather than moved
    /// out: a partial move is planned for a `datac` field of an owned local,
    /// and a payload reached through a `when` arm is neither.
    EnumField {
        base: Box<Expr>,
        variant: usize,
        field: usize,
    },
    /// `expr?` - propagate the failure of an enum shaped `Ok`/`Err` or
    /// `Some`/`None` out of the enclosing function (MK5-S6-M1). `expr` is the
    /// operand, whose own type names the *source* enum; `target` is the
    /// enclosing function's return type, which the failure value is rebuilt
    /// in. Both variant indices are carried so codegen needs no lookup order it
    /// does not already have, and `err_has_payload` is false for `None`, which
    /// carries nothing to move.
    Try {
        expr: Box<Expr>,
        source: String,
        target: String,
        ok_variant: usize,
        err_variant: usize,
        target_err_variant: usize,
        err_has_payload: bool,
        /// A-12 slice 6: when the operand's failure payload type differs from
        /// the enclosing function's, the static `From` method key the failure
        /// is converted through (`AppErr#from@std.convert.From<pkg.IoErr>`),
        /// plus whether the method's parameter is a borrow. A `datac`
        /// parameter is borrowed, so codegen passes the payload's address; a
        /// `Copy` scalar is passed by value. `None` when the two types agree,
        /// which is every `?` written before this existed.
        convert: Option<(String, bool)>,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    Block(Block),
    /// A named function used as a *value* (A-4): `val f = double`. The key is
    /// the declaration's canonical name, so codegen emits `kelp_double` - a
    /// function reference carries no environment, so the C function *is* the
    /// value and there is nothing to allocate or drop.
    FnRef(String),
    /// A `fun` literal that captured something (A-4 phase E): the value is a
    /// *closure* - a function pointer, the heap environment its captures were
    /// moved into, and the environment's drop. `key` names the generated
    /// function (whose signature starts with the hidden environment
    /// parameter), and `captures` holds one initializer per environment field,
    /// in field order, each written in the enclosing function's terms: a
    /// local by name, or a read of an enclosing closure's environment.
    ClosureLit {
        key: String,
        captures: Vec<Expr>,
    },
    /// A scoped `fun` literal (`plan/scoped-blocks.md` part 2): the same fat
    /// pointer as `ClosureLit`, but the environment is written to the
    /// *enclosing function's stack* and the value's `drop` member is `NULL`,
    /// because a scoped literal's captures are copies or shared borrows and
    /// the environment owns nothing. A node of its own so neither backend
    /// mallocs the environment or plans a drop for it. `key` and `captures`
    /// mean exactly what they do on `ClosureLit`.
    ScopedClosureLit {
        key: String,
        captures: Vec<Expr>,
    },
    /// A read of one field of the enclosing closure's environment (A-4 phase
    /// E), inside the generated function that literal became. It is a *place*
    /// (the environment owns the value), which is what lets an owned capture
    /// be borrowed - calling a method on it, passing it to a parameter - while
    /// reading it out as a whole value is the partial move `borrowck.rs`
    /// refuses.
    Capture { index: usize },
    /// A plain function value used where a closure is expected (A-4 phase E):
    /// the widening that makes `val f: closure(i32).return: i32 = double` and
    /// `Holder(cb = fun(x: i32).return: i32 = x)` mean what they read as. A `fun`
    /// value is a bare code pointer with no environment, so the value is
    /// wrapped in a one-field environment holding the pointer, plus a thunk
    /// generated per signature that calls through it. The alternative - a
    /// hidden first parameter on every declared function - is not C's calling
    /// convention, so a wrapper is the honest cost of one type.
    FnToClosure { func: Box<Expr> },
    /// A call through a function value: `f(x)`, where `f` is a local (or,
    /// later, a field) of a `Ty::Fn` type. `callee` is a value expression, so
    /// codegen parenthesizes it before the argument list. Distinct from
    /// `Call`, whose `callee` is a resolved declaration key.
    IndirectCall {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// A borrow: either an explicit `&expr`/`&mut expr` from the surface
    /// syntax, or one synthesized by `infer::check_call_arg` when a plain
    /// place is passed to a `Ty::Ref`-typed parameter (call sites never
    /// need to write `&` themselves - the callee's signature decides).
    Ref {
        mutable: bool,
        expr: Box<Expr>,
    },
    /// A `&T` written where a `&dyn Trait` was expected (A-12 slice 4): the
    /// value is the fat pair `{ data pointer, vtable pointer }`. `expr` is the
    /// `&x` the coercion was written from (a `tir::ExprKind::Ref`), so
    /// `borrowck.rs` sees exactly the borrow the source wrote; `trait_name`
    /// and `type_name` name the vtable the value points at.
    DynCoerce {
        trait_name: String,
        type_name: String,
        expr: Box<Expr>,
    },
    /// A `Box<T>` written where a `Box<dyn Trait>` was expected (A-12 slice 7):
    /// the box *is* the fat pair's data pointer, so this is a move of `expr`
    /// (whose type is `Ty::Box(T)`) with the `(trait, type)` pair's vtable
    /// attached. `borrowck.rs` reads the operand as a whole value, which is
    /// what moves the box and stops its old binding from dropping it.
    BoxDynCoerce {
        trait_name: String,
        type_name: String,
        expr: Box<Expr>,
    },
    /// A `&dyn Trait` borrowed from an owned trait object (A-12 slice 7):
    /// `b.get()` on a `Box<dyn Trait>`. The value *is* the fat pair the box
    /// holds, so no data moves; the node exists so `borrowck.rs` sees the
    /// borrow of the box (the box is read through a borrow, not moved) and so
    /// the resulting `&dyn` keeps the box alive for its own lifetime.
    BoxDynBorrow {
        mutable: bool,
        expr: Box<Expr>,
    },
    /// Whether an owned trait object holds nothing (A-12 slice 7): the box's
    /// data pointer is `NULL`. The value is a `bool`, and the operand is read
    /// as a place.
    BoxDynIsEmpty {
        expr: Box<Expr>,
    },
    /// A method call through a borrowed trait object (A-12 slice 4):
    /// `receiver.vtable[slot](receiver.data, args...)`. `slot` is the method's
    /// index in the trait's flattened vtable, which every vtable of the trait
    /// shares.
    DynCall {
        receiver: Box<Expr>,
        slot: usize,
        args: Vec<Expr>,
        /// The receiver is an *owned* trait object and the method it calls is
        /// `mut Self` (A-12 slice 7): the call writes through the box's data
        /// pointer, so `borrowck.rs` holds an exclusive borrow of the
        /// receiver's own place for the duration of the call. A call through
        /// `&dyn`/`&mut dyn` is `false`: the borrow is already the fat pair the
        /// value is, so the receiver is a `Copy` read.
        exclusive: bool,
    },
    Deref {
        expr: Box<Expr>,
    },
    /// A `print`/`print!` of an interpolated string template, kept as its
    /// pieces so `codegen_c.rs` can emit exactly one `printf` with one
    /// generated format string (rather than concatenating anything, which v1
    /// has no owned string type to do).
    Template {
        /// `true` for `print(...)`, `false` for `print!(...)`.
        newline: bool,
        pieces: Vec<TemplatePiece>,
    },
    /// `[a, b, c]`: an array value (A-18). Every element has the array's own
    /// element type (checked here), and the length is the number of elements,
    /// so the expression's `ty` is a complete `[T; n]` and no later pass has
    /// to count anything.
    ArrayLit(Vec<Expr>),
    /// `[v; N]`: `N` copies of `v`, which is how a zeroed fixed-size buffer is
    /// written (A-18). The length is a compile-time constant and lives in the
    /// expression's own type (`[T; N]`), which is where codegen reads it; the
    /// element must own nothing, because the value is copied into every slot.
    ArrayRepeat { value: Box<Expr> },
    /// `asm("...", operands...)` (kernel stage 5): one instruction of the
    /// target's own assembler. The template's `{k}` placeholders are still the
    /// source spelling; each backend maps them to its own operand numbers.
    Asm(AsmBlock),
}

/// The checked form of one `asm(...)` expression (kernel stage 5).
#[derive(Debug, Clone)]
pub struct AsmBlock {
    /// The target assembler's text, with `{0}`, `{1}`, ... placeholders.
    pub template: String,
    /// The operands, in appearance order (the order `{0}`, `{1}`, ... name).
    pub operands: Vec<AsmOperand>,
    /// The registers the asm clobbers, from `clobber("...")`.
    pub clobbers: Vec<String>,
    /// `nomem`: the asm does not touch memory, so the default `~{memory}`
    /// clobber is dropped.
    pub nomem: bool,
    /// The index in `operands` of the anonymous `out("...") _` whose value the
    /// expression's own value is, when one was written.
    pub value_out: Option<usize>,
}

/// One `in`/`out`/`inout` operand of a checked `asm(...)` (kernel stage 5).
#[derive(Debug, Clone)]
pub struct AsmOperand {
    pub kind: AsmOperandKind,
    /// The constraint the source wrote, passed to the backend unchanged.
    pub constraint: String,
    /// For `in`, the value read; for `out`/`inout`, the place written. `None`
    /// for the anonymous output, which is `AsmBlock::value_out`.
    pub operand: Option<Expr>,
}

/// One piece of a template: literal text, or an interpolated value that has
/// already been checked to be printable.
#[derive(Debug, Clone)]
pub enum TemplatePiece {
    Text(String),
    Value(Expr),
}

/// The bit width of an integer type, as a shift sees it.
pub fn shift_width(ty: &Ty) -> Option<u32> {
    Some(match ty {
        Ty::U8 => 8,
        Ty::U16 => 16,
        Ty::I32 | Ty::U32 => 32,
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => 64,
        _ => return None,
    })
}

/// Whether a shift amount is known to be in `0..bits` without a run-time
/// check: a literal in range, or `x & MASK` with a literal `MASK < bits`
/// (what `wrapping_shl`/`wrapping_shr` lower to). Both backends skip the
/// "shift amount out of range" guard for exactly these.
pub fn shift_amount_in_range(rhs: &Expr, bits: u32) -> bool {
    use crate::ast::BinOp;
    match &rhs.kind {
        ExprKind::IntLit(v) => *v >= 0 && *v < bits as i128,
        ExprKind::Binary { op: BinOp::BitAnd, lhs, rhs } => {
            let lit = |e: &Expr| match &e.kind {
                ExprKind::IntLit(v) => Some(*v),
                _ => None,
            };
            matches!(lit(rhs), Some(m) if m >= 0 && m < bits as i128)
                || matches!(lit(lhs), Some(m) if m >= 0 && m < bits as i128)
        }
        _ => false,
    }
}

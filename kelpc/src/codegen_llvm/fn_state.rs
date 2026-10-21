use super::*;

/// Per-function state: the current block, the entry block, and the local slots.
pub(crate) struct FnGen<'g, 'a> {
    pub(crate) g: &'g Gen<'a>,
    pub(crate) cur_fn: LlvmFunction<'a>,
    pub(crate) entry: BasicBlock<'a>,
    pub(crate) cur: BasicBlock<'a>,
    /// Whether the current block already ends in a terminator, so the walk stops
    /// emitting into it. A `return` terminates; a block after one is dead code.
    pub(crate) terminated: bool,
    /// One map per lexical scope, innermost last, name to alloca slot. A slot is
    /// created in the entry block so `mem2reg` can promote it.
    pub(crate) scopes: Vec<HashMap<String, Value<'a>>>,
    /// The slot each owned binding's declaration made, keyed by the binding the
    /// drop plan recorded. A drop resolves through here rather than through
    /// `scopes`, because one spelling can name several bindings and the drop
    /// belongs to the binding that owns the value (BUG-n7Wq3).
    pub(crate) bindings: HashMap<BindingId, Value<'a>>,
    pub(crate) fn_name: String,
    /// Canonical id of the module the function was declared in. The capability
    /// gate is per module, so a gated call site has to know whose grant it is
    /// checked against (MK7 S3, `Gen::gated_check_needed`).
    pub(crate) module: String,
    pub(crate) ret_ty: Ty,
    /// The function's `tir` body, borrowed for `'a` from the program so the
    /// walk does not hold a borrow of `self`. `None` in the synthetic walk a
    /// `pd` initializer is emitted through, whose single expression is in
    /// `pd_init`.
    pub(crate) body: Option<&'a Block>,
    /// The `pd` initializer this walk emits, when it is the generated
    /// `kelp_init_pds` rather than a function body, and the global it assigns
    /// (`tir::Pd::init`, MK7 §5).
    pub(crate) pd_init: Option<&'a Expr>,
    pub(crate) pd_global: Option<Value<'a>>,
    /// The parameter list, in declaration order (a `fun` literal's environment
    /// first, when it has one).
    pub(crate) params: Vec<(String, Ty)>,
    /// The LLVM environment struct of this literal, when it captured something
    /// (S2E): `Some` makes the generated function take a hidden first `ptr`
    /// parameter and the body's `Capture` reads project into that struct.
    pub(crate) env_ty: Option<Type<'a>>,
    /// The slot holding the hidden environment pointer, bound in the entry
    /// block. One location, so `mem2reg` can promote it out.
    pub(crate) env_slot: Option<Value<'a>>,
    /// The `DISubprogram` this function's source locations hang from, when it
    /// has one (MK7 S4). `None` leaves every instruction of the body without a
    /// location.
    pub(crate) debug_scope: Option<Metadata<'a>>,
    /// The `.kp` span that names this function's own line, which is the
    /// location its entry block starts under. `None` for the synthetic walk a
    /// passdown initializer is emitted through, which has a span of its own
    /// instead.
    pub(crate) decl_span: Option<Span>,
    /// This function's generated cleanup frame (decision 125): a three-word
    /// record the runtime links on the thread's list. `None` unless the program
    /// spawns a worker, and for `naked` and `pd` initializer walks.
    pub(crate) frame: Option<Value<'a>>,
    /// The owned binding slots registered with the frame, in declaration order,
    /// with their types and their liveness flags (an `i8`, 1 while the slot holds
    /// a value the frame owns). The handler drops each one whose flag is set.
    pub(crate) registered: Vec<(Value<'a>, Ty, Value<'a>)>,
    /// The most recently entry-hoisted `alloca`, so the next one chains in
    /// right after it (`alloca`'s own doc comment) instead of always at the
    /// block's absolute start. `None` before the first one.
    pub(crate) alloca_after: Option<Value<'a>>,
    /// `naked fun` (kernel stage 5): the body is one `asm` and the asm itself
    /// returns, so no parameter slots, no `return` of the declared type, and an
    /// `unreachable` terminator after the asm.
    pub(crate) naked: bool,
    /// The loops currently being emitted, innermost last: the block a
    /// `continue` branches to (the loop's condition) and the block a `break`
    /// branches to (the loop's exit). `kelp wiki page language/iterators.md` adds the two loop
    /// control statements, and this is what gives them somewhere to go.
    pub(crate) loops: Vec<(BasicBlock<'a>, BasicBlock<'a>)>,
    /// How many `break` statements have been emitted so far in this function;
    /// `emit_while` compares it before and after a `while true` body to know
    /// whether the loop has an exit edge at all.
    pub(crate) breaks_emitted: usize,
    /// The manual block regions being emitted, innermost last (decision 139).
    /// Each one's release runs when its body ends.
    pub(crate) regions: Vec<RegionSlot<'a>>,
    /// The `regions` depth at each loop being emitted, parallel to `loops`: a
    /// `break` or `continue` releases the regions opened inside its loop.
    pub(crate) loop_regions: Vec<usize>,
}

/// One manual block region being emitted: its name (for the names of its
/// instructions), and the two slots that hold the block's element address and
/// its root handle. The slots are null and zero until a `name.box()` fills them.
#[derive(Clone)]
pub(crate) struct RegionSlot<'a> {
    pub(crate) name: String,
    pub(crate) elem: Ty,
    pub(crate) base: Value<'a>,
    pub(crate) handle: Value<'a>,
}

// ===========================================================================
// The containers (MK7 S2D): `List<T>`, `Map<K, V>`, `Box<T>` and `[T; N]`.
//
// MK7 §3.2 names the exception this section exists for: the fixed runtime stays
// C, but the monomorphized container code cannot, because the earlier emitter
// generates one function set per element or value type. So this backend
// generates the same sets into the module, one per instantiation, with the same
// function names and the same bodies the earlier emitter writes. A container's header
// is a named struct whose field order and widths match the C typedefs field for
// field, but it never crosses to C by value: the runtime helpers it calls take
// a pointer (MK7 §3.3 and BUG-vN4tR).
// ===========================================================================

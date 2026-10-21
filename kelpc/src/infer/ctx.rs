use super::*;

/// Cloned once per function body checked, and once more per `fun` literal
/// (A-4), whose body is checked in a scope of its own with the same module
/// tables. The two `Rc` fields are what make a clone share state: a generic
/// instantiation (or a literal's synthesized function) created while checking
/// one clone is visible to every other.
#[derive(Clone)]
pub(crate) struct Ctx {
    /// Manual block regions (decision 139): every block name declared so far in
    /// this function. A name is unique per function, so a second block of the
    /// same name, or a let of that name, is refused by name.
    pub(crate) block_names: std::collections::HashSet<String>,
    /// The blocks being checked, innermost last: name and element type. A
    /// `name.box()` is valid only while its block is active.
    pub(crate) active_blocks: Vec<(String, Ty)>,
    /// Let-bound names so far in this function (the shadow check).
    pub(crate) let_names: std::collections::HashSet<String>,
    /// Let-bound block boxes in scope, each with the name of its block. Such a
    /// binding may only be read as the base of a field access, because the
    /// region releases its element.
    pub(crate) box_bindings: HashMap<String, String>,
    /// The block boxes of the enclosing bodies, as seen from inside a `fun`
    /// literal. A literal that names one of them captures a block value, which
    /// is refused (decision 139, step 4).
    pub(crate) outer_boxes: HashMap<String, String>,
    /// Set just before the operand of a `let`, a call, an assignment or a block
    /// tail when that operand is a bare name: where the value of a block box
    /// would go if the name were allowed there. Read by the next expression
    /// check, and used only to word the refusal.
    pub(crate) box_sink_ok: Option<BoxSink>,
    pub(crate) box_sink_here: Option<BoxSink>,
    /// Set just before a block expression that is the direct operand of `?` or
    /// `when`: the only places a block result may go (decision 139, R31).
    pub(crate) block_result_used: bool,
    /// Set just before the initializer of a `let` that is `name.box()?`.
    pub(crate) box_let_ok: bool,
    /// Set by a field access for its base, read by the identifier check.
    pub(crate) field_base_ok: bool,
    /// How many `name.box()` calls each block has in this function. A block
    /// holds one element, so a second box is refused (slice 1).
    pub(crate) block_box_count: HashMap<String, usize>,
    pub(crate) field_base_here: bool,
    pub(crate) sigs: HashMap<String, FnSig>,
    pub(crate) structs: HashMap<String, StructDef>,
    /// The `enum` table (A-1), keyed exactly like `structs`. An enum name is
    /// also present in `structs` (as a definition with no fields) purely so
    /// that one name-resolution path covers both kinds of user type; this
    /// table is what construction, pattern matching and the `has no fields`
    /// diagnostics consult.
    pub(crate) enums: HashMap<String, EnumDef>,
    /// Top-level `const`s (M6.3), keyed by their module-qualified name and
    /// already folded to a literal - so a use site is a substitution, not a
    /// load from anywhere.
    pub(crate) consts: HashMap<String, ConstInfo>,
    /// Top-level `static`s (kernel stage 7), keyed by their module-qualified
    /// name. A static is a real object, so a use site is a `StaticRef` place
    /// rather than a substitution, and the type and mutability travel with it.
    pub(crate) statics: HashMap<String, StaticInfo>,
    /// The functions a compile-time fold may call (A-19). Shared rather than
    /// cloned, because a `Ctx` is cloned once per body checked and this table
    /// is the whole program's functions.
    pub(crate) const_fns: std::rc::Rc<ConstFns>,
    /// Which user types own heap memory, transitively (`types::owned_types`).
    /// A `[T; N]` repeat literal (`[Gate(...); 256]`) copies its value into
    /// every element, so it needs the same answer the drop planner does: does
    /// this type own anything (A-18)?
    pub(crate) owned_structs: std::collections::HashSet<String>,
    /// Overload sets for compiler-shipped stdlib functions (M6.3): plain key
    /// (`std.math.abs`) -> each declaration's parameter types and its
    /// type-suffixed key.
    pub(crate) overloads: HashMap<String, Vec<(Vec<Ty>, String)>>,
    /// The module being checked - needed by capability diagnostics, which
    /// have to name the module that lacks the grant.
    pub(crate) module: String,
    /// Capability domains *this* module holds, with their patterns (M7). The
    /// gate below is what makes an ungranted `fs.read` a resolution error
    /// rather than a runtime check that happens to reject it.
    pub(crate) caps: HashMap<String, Vec<String>>,
    /// Function key -> gated domain, for the modules that gate one.
    pub(crate) gated: HashMap<String, String>,
    /// Whether this module's `coral.toml` carries an `[extern."<module id>"]`
    /// section. A handle operation (`null()`, `is_null()`, `cast()`) is legal
    /// only where one exists, even when the handle type is imported from an
    /// extern module.
    pub(crate) has_extern_section: bool,
    /// The program's variadic user `extern fun`s (gap 7): canonical key ->
    /// declaring module. A call site reads it to know a callee takes extra
    /// arguments and which wrapper to name.
    pub(crate) variadic_externs: std::rc::Rc<HashMap<String, String>>,
    pub(crate) ret_ty: Ty,
    /// The name of the function whose body is being checked, for the messages
    /// that name it (a block's `?` in a function that returns `Unit`).
    pub(crate) fn_name: String,
    pub(crate) scopes: Vec<HashMap<String, VarInfo>>,
    /// Symbol prefix of the module being checked (`""` for the entry
    /// module, else `"shapes.circle."`) - a bare type name in this module
    /// resolves to `prefix + name`.
    pub(crate) prefix: String,
    /// alias -> target module's symbol prefix, for this module's `impl`
    /// lines (M5). This is also how a module-qualified *call* is
    /// recognized: `circle.make(...)` is a module item reference, not a
    /// method call, when `circle` is one of these aliases.
    pub(crate) aliases: HashMap<String, String>,
    /// This module's passdowns, keyed by the bare name a function writes.
    /// `design/syntax.md (kelp wiki)` §9 leaves the cross-module question open; M5.2
    /// answers it the same way M5.1 answered imports - module-scoped, so
    /// only functions in the declaring module can name it.
    pub(crate) pds: HashMap<String, PdInfo>,
    /// The program's generic type templates and the instances built so far
    /// (A-2). `structs`/`enums` hold the hand-written declarations; an
    /// instantiation lives in `env.instances` and is reached through
    /// `struct_def`/`enum_def`.
    pub(crate) env: TypeEnv,
    /// The program's generic function templates and their instantiated
    /// signatures (A-2). `sigs` holds the hand-written, non-generic
    /// declarations; an instantiated one lives in `fns.instances` and is
    /// reached through `sig_of`.
    pub(crate) fns: FuncEnv,
    /// The type parameters in scope while a generic function's body is being
    /// checked, bound to the instantiation's arguments. Empty everywhere
    /// else, which is what `type_scope` reads.
    pub(crate) type_args: HashMap<String, Ty>,
    /// Passdowns of *other* modules, bare name -> declaring module. Used for
    /// diagnostics: naming one is an error, and the message says why.
    pub(crate) foreign_pds: HashMap<String, String>,
    /// sig key -> passdowns that callee references, in declaration order:
    /// what every call site has to pass as hidden arguments.
    pub(crate) wired: HashMap<String, Vec<PdInfo>>,
    /// While a `pd` initializer is being checked: the hidden names already
    /// initialized. `None` inside a function body, where every passdown is
    /// available. This is what stops an initializer from reading a
    /// passdown that is still zero-initialized at that point.
    pub(crate) init_state: Option<std::collections::HashSet<String>>,
    /// True only while checking `main`'s body. `main` is emitted with a
    /// fixed C signature - it cannot receive hidden parameters - so its
    /// passdown references take the address of the generated global instead
    /// of naming a parameter. Every other function that references a
    /// passdown is wired and uses its parameter.
    pub(crate) pd_global_borrow: bool,
    /// The hidden passdown parameters *this* function received, by their
    /// internal name (`pd#<qualified>`). The generated parameter and the
    /// generated global share that name, so an identifier in the body means
    /// the parameter when it is listed here and the file-scope global when it
    /// is not - a distinction `hidden_args` has to make when it hands a
    /// passdown on: a parameter is already the borrow, while the global needs
    /// its address taken. Empty for `main`, a `pd` initializer, and a `fun`
    /// literal, none of which can receive one.
    pub(crate) own_hidden: Vec<String>,
    /// The wiring key this body is checked under - `main`, a declaration key,
    /// or the template an instantiation came from. The borrow modes and the
    /// probe's collected uses are both filed under it.
    pub(crate) fn_key: String,
    /// The passdown borrow modes this body check runs under (roadmap item
    /// 17). Empty in the probe pass, where every passdown is shared and a
    /// write is recorded instead of refused; `pd_global_borrow` bodies
    /// (`main`, a `pd` initializer) are filed under their own key, so a
    /// `main` that only reads a `pd var` takes a shared borrow of it too.
    pub(crate) pd_modes: std::rc::Rc<HashMap<(String, String), bool>>,
    /// Set only in the probe pass: where an exclusive use is recorded and the
    /// call edges are collected (see `PdProbe`).
    pub(crate) pd_probe: Option<std::rc::Rc<std::cell::RefCell<PdProbe>>>,
    /// True while checking the body of a `fun` literal (A-4). A literal is
    /// its own function, so two diagnostics need to know: a name it does not
    /// bind itself is a *capture*, and a passdown is a hidden argument it
    /// cannot receive.
    pub(crate) in_fn_literal: bool,
    /// The *types* of the bindings that were in scope where the literal
    /// currently being checked was written (A-4 phase E), plus whatever the
    /// enclosing literal had already captured. A name the literal's own body
    /// does not bind that appears here is a capture; a name that appears
    /// nowhere is a typo or a missing `impl`, and the two read differently.
    pub(crate) outer_bindings: HashMap<String, Ty>,
    /// The captures this literal has collected so far, in the order its body
    /// first named them (A-4 phase E). The body check fills this in, and
    /// `check_fn_literal` reads it afterwards to decide whether the literal
    /// is a plain function value or a closure, and what its environment
    /// holds. Empty outside a literal body, which is also how a nested
    /// literal finds the *enclosing* literal's captures (indices into the
    /// enclosing environment).
    pub(crate) captures: Vec<(String, Ty)>,
    /// True while checking a *scoped* literal (`kelp wiki page language/functions.md`),
    /// whose environment is stack and whose captures may be borrows: an
    /// owned non-`Copy` enclosing binding is captured as a shared borrow
    /// rather than moved, so `note_capture` records `&T` and the body reads
    /// the capture through it.
    pub(crate) borrowed_captures: bool,
    /// True while checking a `fun` literal written directly as the `thread.spawn`
    /// entry (decision 127): its own captures are checked by the crossing rule,
    /// because the worker's cleanup drops the environment on every exit. Only the
    /// literal's own captures see it; a literal nested inside gets its own value.
    pub(crate) spawn_entry: bool,
    /// Set by `check_thread_spawn` just before it checks a `fun` literal entry,
    /// and taken by the next literal pass, which hands it to `spawn_entry`.
    pub(crate) spawn_entry_next: bool,
    /// The enclosing owned `var` bindings a scoped literal may capture by
    /// *exclusive* borrow (`kelp wiki page language/functions.md`): names that are
    /// mutable, owned and not `Copy`.
    pub(crate) outer_mut: std::collections::HashSet<String>,
    /// This pass's upgrades: the captures `note_capture` types `&mut T`
    /// instead of `&T`. Grown by `check_fn_literal`'s retry loop.
    pub(crate) excl_captures: std::collections::HashSet<String>,
    /// The names an *enclosing* scoped literal already captures exclusively:
    /// a block inside that block cannot hold them (v1, `kelp wiki page language/functions.md`).
    pub(crate) outer_excl: std::collections::HashSet<String>,
    /// The captures this pass wrote through (or passed as `mut`) while typed
    /// as a shared borrow: what the retry loop upgrades. One cell per pass.
    pub(crate) excl_needed: std::rc::Rc<std::cell::RefCell<std::collections::BTreeSet<String>>>,
    /// The `datac` whose body the literal was written in, when the literal is
    /// inside a receiver-form method (A-4 phase E). Its fields are names the
    /// literal can *see* but cannot capture: reading one out of a borrowed
    /// receiver is a partial move (A-11), so the diagnostic says that instead
    /// of reporting an undefined name.
    pub(crate) outer_receiver: Option<String>,
    /// Set while checking a receiver-form function whose receiver resolves
    /// to a `datac`: lets a bare identifier that isn't a local fall back to
    /// an implicit `self.field` (`fun Circle.area() => f64 = 3.14159 * r *
    /// r` - no `self.` qualifier written, per `design/syntax.md (kelp wiki)` §5's "no
    /// `self` keyword").
    pub(crate) receiver_struct: Option<String>,
    /// How many scrutinee temporaries a `when` has staged, so each one's
    /// synthesized binding has a name of its own (`infer::WHEN_TEMP_PREFIX`).
    pub(crate) when_temps: usize,
    /// How many `for` loops this body has desugared, so each loop's
    /// synthesized sequence and index names are unique (`kelp wiki page language/iterators.md`).
    /// A `fun` literal's body starts a fresh count, exactly as a function
    /// does.
    pub(crate) for_temps: usize,
    /// How many loops enclose the statement being checked. A `break` or
    /// `continue` outside every loop is refused here rather than reaching a
    /// backend that has no loop to branch out of.
    pub(crate) loop_depth: usize,
    /// The same counter for a destructuring binding's value and its nested
    /// levels (`infer::TUPLE_TEMP_PREFIX`).
    pub(crate) tuple_temps: usize,
    /// Set only while the initializer of a `let` with no written type is being
    /// checked. A bare `List.empty()`/`Box.empty()`/`Map.new()` there leaves
    /// its element type open instead of failing at the declaration, for the
    /// first `push`/`insert` on the binding to decide (: the marker and the
    /// repair are `types::is_open_container` and `check_block`). Anywhere
    /// else an empty container with no type to take its element from is a
    /// declaration-time error, exactly as it always was.
    pub(crate) defer_empty_container: bool,
    /// Expressions already checked once with no (or a hint-only) expectation to
    /// learn a type (a method call's receiver, a generic call's arguments),
    /// kept for the `check_call_arg` that follows so they are not checked a
    /// second time: nested calls would otherwise double their work at every
    /// level. Keyed by the expression's address and span; an entry is used
    /// only when its type is exactly the one the argument is then expected to
    /// have, and is dropped when used.
    pub(crate) probes: Vec<(*const ast::Expr, Span, tir::Expr)>,
    /// Set by the one call that is about to match its arguments against a
    /// signature (`match_args_split` takes it at once): whether a `String`
    /// argument for a `str` parameter is borrowed for the call, or refused by
    /// name. Unset means no such conversion, as for a constructor or a trait
    /// static call.
    pub(crate) str_arg_site: Option<StrArgSite>,
    /// Set by a builtin call that stores its arguments into a shared container
    /// (`xs.push`, `m.insert`, `b.set`), named by the receiver's root: the next
    /// `match_args_split` takes it, so a block value passed there is refused
    /// with the store wording (golden G1) instead of the move wording.
    pub(crate) box_store_into: Option<String>,
    /// How many `unsafe { ... }` blocks enclose the expression being checked
    /// (MK9-S2 stage 1). Non-zero means the unsafe-only operations are legal;
    /// the body of an `unsafe fun` starts at 1.
    pub(crate) unsafe_depth: usize,
    /// The functions declared `unsafe fun`, by canonical key. Calling one
    /// requires `unsafe_depth > 0`.
    pub(crate) unsafe_fns: std::collections::HashSet<String>,
    /// The functions declared `must_use fun`, by canonical key: a statement
    /// that is a call to one, with its result unread, is refused.
    pub(crate) must_use_fns: std::collections::HashSet<String>,
    /// Whether the project is a freestanding build (kernel stage 8). The
    /// `percpu` storage class and its two intrinsics exist only there.
    pub(crate) freestanding: bool,
    /// The manifest's `[target] cpus` (kernel stage 8), which is the value
    /// `cpu_count()` folds to and the number of copies a `percpu` static has.
    pub(crate) cpus: u64,
    /// Set only while checking the block argument of `Mutex.lock` (B-5): a
    /// scoped block's return type is the block's own tail type, so
    /// `check_fn_literal` infers it instead of taking it from the expected
    /// `scoped fun(...) => R`. Cleared for the body of the literal itself, so
    /// a nested scoped block is typed by its own callee again.
    pub(crate) infer_scoped_ret: bool,
}

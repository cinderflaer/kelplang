use crate::ast::Repr;

mod ty;
pub use ty::*;

/// `T, U` for a diagnostic or a written type.
fn join_types(types: &[Ty]) -> String {
    types.iter().map(|t| t.display()).collect::<Vec<_>>().join(", ")
}

/// The mangled signature fragment a closure instantiation is named by:
/// `i32__i32` is `closure(i32) => i32`. Parameters first, in order, then the
/// return - the same order `ty_mangle`'s function-type arm uses, under a
/// different prefix.
pub fn closure_mangle(params: &[Ty], ret: &Ty) -> String {
    format!(
        "Closure__{}{}",
        params.iter().map(|p| format!("{}__", ty_mangle(p))).collect::<String>(),
        ty_mangle(ret)
    )
}

/// The `(params, ret)` of a closure-shaped signature, whichever of the three
/// spellings it is (`fun`, `closure`, `scoped fun`). `None` for anything else.
pub fn closure_parts(ty: &Ty) -> Option<(&[Ty], &Ty)> {
    match ty {
        Ty::Fn { params, ret }
        | Ty::Closure { params, ret }
        | Ty::ScopedFn { params, ret } => Some((params.as_slice(), ret)),
        _ => None,
    }
}

/// The `Ty::Closure` spelling of a closure-shaped signature, so the body emitted
/// for a `scoped fun(...)` block shares the key a thread's `closure(...)` entry
/// uses (B-5).
pub fn closure_shape(ty: &Ty) -> Ty {
    match ty {
        Ty::Fn { params, ret }
        | Ty::Closure { params, ret }
        | Ty::ScopedFn { params, ret } => Ty::Closure {
            params: params.clone(),
            ret: ret.clone(),
        },
        other => other.clone(),
    }
}

/// The generated C struct tag for a closure instantiation (A-4 phase E):
/// `{ fn, env, drop }`, where `fn` takes the environment as its hidden first
/// parameter. Monomorphized per signature, like a container, so the C
/// function-pointer member is typed rather than `void*`.
pub fn closure_c_type(params: &[Ty], ret: &Ty) -> String {
    format!("kelp_{}", closure_mangle(params, ret))
}

/// The name of one field of a literal's environment (`c0`, `c1`, …). One
/// place names them, because `infer.rs` writes the environment's field list
/// and the removed C backend emitted the reads.
pub fn capture_field(index: usize) -> String {
    format!("c{index}")
}

/// The generated C struct tag for a literal's environment, and the function
/// that frees it (A-4 phase E). One place names both, because `infer.rs`
/// records them on the generated function and the removed C backend emitted them.
pub fn env_struct_name(literal_key: &str) -> String {
    format!("kelp_Env__{}", c_ident(literal_key))
}

/// See `env_struct_name`.
pub fn env_drop_name(literal_key: &str) -> String {
    format!("kelp_env_drop__{}", c_ident(literal_key))
}

/// Which user-declared types own heap memory, transitively: a `datac` whose
/// field owns memory, or an `enum` with one such payload in any variant.
///
/// A type may only name a type declared before it *by value* (open item 12),
/// so one pass in merged declaration order is already a valid fixpoint - no
/// repeating until stable. Merged order matters here for the first time in
/// A-1: a `datac` and an `enum` may each name the other's earlier-declared
/// half, so the two declaration lists have to be walked as one sequence.
///
/// `drop_types` is the set of user types that declare their own destructor
/// (kernel stage 9, MK9-S6-M1); each is seeded into the result, because a
/// destructor type needs a `kelp_drop_<T>` to hold the user call even when it
/// owns no memory, and a later type that holds one by value then owns memory
/// through it.
pub fn owned_types(
    structs: &[StructDef],
    enums: &[EnumDef],
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    enum Decl<'a> {
        Record(&'a StructDef),
        Sum(&'a EnumDef),
    }
    let mut decls: Vec<(usize, Decl)> = structs
        .iter()
        .map(|s| (s.order, Decl::Record(s)))
        .chain(enums.iter().map(|e| (e.order, Decl::Sum(e))))
        .collect();
    decls.sort_by_key(|(order, _)| *order);

    let mut owned = drop_types.clone();
    for (_, decl) in decls {
        let (name, fields_own) = match decl {
            Decl::Record(s) => (
                &s.name,
                s.fields.iter().any(|f| needs_drop(&f.ty, &owned)),
            ),
            // An `enum` owns memory as soon as *one* variant does: the drop
            // function switches on the live tag, so any owned payload means
            // the value needs a drop call.
            Decl::Sum(e) => (
                &e.name,
                e.variants
                    .iter()
                    .flat_map(|v| v.fields.iter())
                    .any(|f| needs_drop(&f.ty, &owned)),
            ),
        };
        if fields_own {
            owned.insert(name.clone());
        }
    }
    owned
}

/// Whether a `List<T>` element type has an order, so `List.sort` means
/// something. Numbers and `bool` compare by value; `String`/`Bytes` compare
/// byte-wise (length-aware, not `strcmp` - an embedded NUL is possible). A
/// `datac` element has no defined order, and a borrow can never be an element
/// at all.
pub fn list_sortable_elem(ty: &Ty) -> bool {
    ty.is_numeric() || matches!(ty, Ty::Bool | Ty::String | Ty::Bytes)
}
/// Whether a `List<T>` element type has equality, so `List.contains` means
/// something. Same set as `list_sortable_elem`, for the same reason: a
/// `datac` has no `==` in v1.
pub fn list_comparable_elem(ty: &Ty) -> bool {
    list_sortable_elem(ty)
}

/// Whether one field, payload or `List` element of type `ty` can be deep-copied
/// once its user types can. `clonable` holds the `datac`/`enum` names that can
/// be (see `clonable_types`). This is the single gate for every copy *out* of a
/// container, `List.slice`/`List.clone` and `Map.get_or`/`Map.values` alike.
pub fn value_is_clonable(ty: &Ty, clonable: &std::collections::HashSet<String>) -> bool {
    ty.is_copy()
        || matches!(ty, Ty::String | Ty::Bytes)
        || matches!(ty, Ty::Struct(n) if clonable.contains(n))
}

/// Whether one field or payload of type `ty` has an order, once its user types
/// have. `String`/`Bytes` compare byte-wise and length-aware; a `datac`/`enum`
/// compares field by field in declaration order.
pub fn value_is_sortable(ty: &Ty, sortable: &std::collections::HashSet<String>) -> bool {
    ty.is_numeric()
        || matches!(ty, Ty::Bool | Ty::Char | Ty::String | Ty::Bytes)
        || matches!(ty, Ty::Struct(n) if sortable.contains(n))
}

/// Whether one field, payload or `datac`/`enum` value of type `ty` can be a
/// `Map` key once its user types can. This is the derived key set: the same
/// scalars `is_map_key` names, plus a `datac`/`enum` every one of whose fields
/// or variant payloads is itself keyable.
pub fn value_is_keyable(ty: &Ty, keyable: &std::collections::HashSet<String>) -> bool {
    is_map_key(ty) || matches!(ty, Ty::Struct(n) if keyable.contains(n))
}

/// The user types whose value is a key *by derivation* (A-6): the least
/// fixpoint in which a `datac`/`enum` is keyable once every field or payload it
/// holds is. The same shape as `clonable_types`/`sortable_types`, over the same
/// declaration graph, minus the types in `declared`.
///
/// `declared` names the types that declare their own `hash`/`eq` pair (A-12).
/// A declared pair is used *instead of* a derived hash and equality, so such a
/// type is never in this set, exactly as if it were not keyable at all: the
/// derived pair the backends emit is decided by this set alone, and a field
/// that names one of these types is therefore not keyable by derivation either.
pub fn keyable_types(
    decls: &[(String, Vec<Ty>)],
    declared: &std::collections::HashSet<String>,
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let plain: Vec<(String, Vec<Ty>)> = decls
        .iter()
        .filter(|(name, _)| !declared.contains(name) && !drop_types.contains(name))
        .cloned()
        .collect();
    fixpoint(&plain, value_is_keyable)
}

/// The names a `Map` key may be, as `is_map_key_with` asks the question: the
/// derived key set plus the types that declare their own pair (A-12). A `Map`
/// takes a derived key and a declaring one, so this set is the union of the
/// two.
pub fn map_key_types(
    decls: &[(String, Vec<Ty>)],
    declared: &std::collections::HashSet<String>,
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let mut set = keyable_types(decls, declared, drop_types);
    set.extend(declared.iter().filter(|n| !drop_types.contains(*n)).cloned());
    set
}

/// The user types whose value can be deep-copied: the least fixpoint in which a
/// `datac`/`enum` is clonable once every field or payload it holds is. `decls`
/// is one `(name, each field's type)` per declaration, including generic
/// instances.
///
/// A type that reaches itself by value stays out of the set, which is the
/// honest answer: v1's declaration rule already forbids a directly recursive
/// `datac`, and a recursion through a `Box`/`List` is not a shape a plain copy
/// can follow without a decision about what the copy owns.
pub fn clonable_types(
    decls: &[(String, Vec<Ty>)],
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let plain: Vec<(String, Vec<Ty>)> = decls
        .iter()
        .filter(|(name, _)| !drop_types.contains(name))
        .cloned()
        .collect();
    fixpoint(&plain, value_is_clonable)
}

/// The same fixpoint for the order a `datac`/`enum` gets when it is sorted:
/// fields in declaration order, a `String`/`Bytes` byte-wise, an `enum` by tag
/// and then by the live variant's payload. A `List`/`Box`/`Map` field is not
/// ordered, so a record that holds one is not sortable either.
pub fn sortable_types(decls: &[(String, Vec<Ty>)]) -> std::collections::HashSet<String> {
    fixpoint(decls, value_is_sortable)
}

fn fixpoint(
    decls: &[(String, Vec<Ty>)],
    ok: fn(&Ty, &std::collections::HashSet<String>) -> bool,
) -> std::collections::HashSet<String> {
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let mut changed = false;
        for (name, fields) in decls {
            if set.contains(name) {
                continue;
            }
            if fields.iter().all(|f| ok(f, &set)) {
                set.insert(name.clone());
                changed = true;
            }
        }
        if !changed {
            return set;
        }
    }
}

/// The generated C function that deep-copies a `datac`/`enum` from `src` into
/// `dst` (the caller's storage, which must not already own anything).
pub fn clone_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_clone_{}", c_ident(name)),
        other => unreachable!("no clone rule for type '{}'", other.display()),
    }
}

/// The generated C function that orders two values of a `datac`/`enum`: the
/// sign of the difference, like `memcmp`.
pub fn cmp_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_cmp_{}", c_ident(name)),
        other => unreachable!("no order rule for type '{}'", other.display()),
    }
}

/// The generated function that hashes a keyable `datac`/`enum` key: the
/// combine helper folded over the tag (an `enum`) and then each field.
pub fn hash_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_hash_{}", c_ident(name)),
        other => unreachable!("no hash rule for type '{}'", other.display()),
    }
}

/// The generated function that tests two keyable `datac`/`enum` keys for
/// equality, the authority the map's probe consults.
pub fn eq_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_eq_{}", c_ident(name)),
        other => unreachable!("no equality rule for type '{}'", other.display()),
    }
}

/// The two method names a `datac`/`enum` declares to be its own `Map` key pair
/// (A-12): `fun T.hash() => u64` and `fun T.eq(other: &T) => bool`. The names
/// belong to the pair rather than to the type, which is why declaring either
/// one is read as a statement about the type's key.
pub const KEY_PAIR_HASH_METHOD: &str = "hash";
pub const KEY_PAIR_EQ_METHOD: &str = "eq";

/// The canonical key of one trait *instance* (A-12 slice 6): the trait's
/// canonical name, plus its arguments when it has any (`pkg.From<pkg.IoErr>`).
/// A trait with no type parameters keys by its name alone, exactly as it did
/// before generic traits existed, so nothing about an ordinary trait changes.
/// The key is also what a static impl method's function name is suffixed with,
/// which is how `From<IoErr>`'s `from` and `From<String>`'s `from` for one type
/// stay distinct.
/// The receiver name every method of a blanket impl (`impl<T: Bound> Trait for
/// T`) is keyed under: `impl#Trait`, which no declared type can carry because
/// `impl` is a keyword. A blanket method's key is `impl#Trait#name`.
pub fn blanket_receiver_name(trait_key: &str) -> String {
    format!("impl#{trait_key}")
}

/// Whether a receiver type name is a blanket impl's (see `blanket_receiver_name`).
pub fn is_blanket_receiver(name: &str) -> bool {
    name.starts_with("impl#")
}

pub fn trait_instance_key(name: &str, args: &[String]) -> String {
    if args.is_empty() {
        name.to_string()
    } else {
        format!("{name}<{}>", args.join(","))
    }
}

/// One half of a declared key pair as `sigs`/`tir` spell it: the `Type#name`
/// key `infer.rs`'s `sig_key` gives every receiver method. The `Map`'s call
/// target is this key mangled by `c_ident`, so it names the method's own
/// declaration and the two cannot drift.
pub fn key_pair_method_key(type_name: &str, method: &str) -> String {
    format!("{}#{}", type_name, method)
}

/// The destructor's method name (kernel stage 9, MK9-S6-M1). Like the two
/// halves of a key pair, the name *is* the declaration: `fun mut T.drop()` on
/// a user type makes that type's own drop run at every drop point. Both
/// backends reach the body through the same `Type#name` key `sig_key` gives
/// every receiver method.
pub const DROP_METHOD: &str = "drop";

/// The `sigs`/`tir` key of a type's declared destructor.
pub fn drop_method_key(type_name: &str) -> String {
    format!("{}#{}", type_name, DROP_METHOD)
}

/// The canonical names of the shipped operator traits (`std/ops`, language-
/// completeness 5). `infer.rs` lowers `+`/`-`/`*`/`/`/`%`, unary `-` and the
/// order operators through these, so the name is a fixed spelling rather than
/// "whatever trait declares a method called `add`".
pub const ADD_TRAIT: &str = "std.ops.Add";
pub const SUB_TRAIT: &str = "std.ops.Sub";
pub const MUL_TRAIT: &str = "std.ops.Mul";
pub const DIV_TRAIT: &str = "std.ops.Div";
pub const REM_TRAIT: &str = "std.ops.Rem";
pub const NEG_TRAIT: &str = "std.ops.Neg";
pub const ORD_TRAIT: &str = "std.ops.Ord";

/// The canonical name of `std/fmt`'s `Display`, the trait a value implements
/// to appear in `"${x}"` and as the argument of `print`/`print!`.
pub const DISPLAY_TRAIT: &str = "std.fmt.Display";

/// Whether `ty` is a user type that declares its own destructor (kernel stage
/// 9, MK9-S6-M1). A partial move of a field or payload of such a type is
/// refused by `borrowck.rs`: the move zeroes the slot it came from and the
/// base's own drop still calls the generated drop on the zero value, which for
/// a destructor type would run the user body a second time.
pub fn is_drop_type(ty: &Ty, drop_types: &std::collections::HashSet<String>) -> bool {
    matches!(ty, Ty::Struct(name) if drop_types.contains(name))
}

/// Whether a value of `ty` has to be freed - the builtin owned types, or a
/// `datac` that (transitively) holds one.
pub fn needs_drop(ty: &Ty, owned_structs: &std::collections::HashSet<String>) -> bool {
    match ty {
        Ty::Struct(name) => owned_structs.contains(name),
        // An array is freed element by element, so it needs a drop call
        // exactly when its element does.
        Ty::Array(elem, _) => needs_drop(elem, owned_structs),
        other => other.owns_heap(),
    }
}

/// The C function that frees a value of `ty`, taking a pointer to it.
/// Generated per `datac` by the removed C backend; the builtins are in the fixed
/// runtime preamble.
pub fn drop_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Bytes => "kelp_bytes_drop".to_string(),
        Ty::String => "kelp_string_drop".to_string(),
        // One runtime function for every element type: a handle detaches the
        // worker and frees nothing, so the element type is not part of the
        // step (MK9-S3-M1 stage C1).
        Ty::Thread(_) => "kelp_thread_drop".to_string(),
        // One runtime function for every element type: the record carries the
        // element drop pointer `Mutex.new` stored, so the element type is not
        // part of the step (B-5).
        Ty::Mutex(_) => "kelp_mutex_drop".to_string(),
        Ty::RwLock(_) => "kelp_rwlock_drop".to_string(),
        Ty::Condvar => "kelp_condvar_drop".to_string(),
        Ty::Struct(name) => format!("kelp_drop_{}", c_ident(name)),
        Ty::List(elem) => format!("kelp_list_drop__{}", ty_mangle(elem)),
        Ty::Box(elem) => format!("kelp_box_drop__{}", ty_mangle(elem)),
        // One drop per trait: the value behind the data pointer is dropped
        // through the vtable's own drop entry, so the concrete type is not part
        // of the function's name (A-12 slice 7).
        Ty::BoxDyn { trait_name } => box_dyn_drop_fn(trait_name),
        Ty::Array(elem, len) => format!("kelp_array_drop__{}__{}", ty_mangle(elem), len),
        Ty::Map(key, value) => {
            format!("kelp_map_drop__{}__{}", ty_mangle(key), ty_mangle(value))
        }
        Ty::Closure { params, ret } => {
            format!("kelp_closure_drop__{}", closure_mangle(params, ret))
        }
        // A scoped function value owns nothing (its environment is stack and
        // its drop is NULL), so it has no drop function to name.
        other => unreachable!("no drop rule for type '{}'", other.display()),
    }
}

mod elements;
pub use elements::*;

/// The C name fragment for a container's element type: `List<i32>` becomes
/// `i32`, `List<shapes.circle.Circle>` becomes `shapes__circle__Circle`
/// (through the same `c_ident` mangling every other generated symbol uses),
/// and a nested container nests the fragment (`Box<List<i32>>` becomes
/// `Box__List__i32`). One function for both containers, so a name can never
/// collide between them and a nested one cannot produce an invalid C
/// identifier.
pub fn ty_mangle(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => c_ident(name),
        Ty::Bytes => "Bytes".to_string(),
        Ty::String => "String".to_string(),
        Ty::List(elem) => format!("List__{}", ty_mangle(elem)),
        Ty::Box(elem) => format!("Box__{}", ty_mangle(elem)),
        Ty::Map(key, value) => format!("Map__{}__{}", ty_mangle(key), ty_mangle(value)),
        Ty::Array(elem, len) => format!("Array__{}__{}", ty_mangle(elem), len),
        Ty::ConstInt { .. } => {
            unreachable!("a const argument has no mangled type; it is substituted before codegen")
        }
        Ty::Atomic(elem) => format!("Atomic__{}", ty_mangle(elem)),
        Ty::Thread(elem) => format!("Thread__{}", ty_mangle(elem)),
        Ty::Mutex(elem) => format!("Mutex__{}", ty_mangle(elem)),
        Ty::RwLock(elem) => format!("RwLock__{}", ty_mangle(elem)),
        Ty::Condvar => "Condvar".to_string(),
        // The canonical name is module-qualified (`ffi.Widget`), so it goes
        // through the same mangling every other generated symbol uses rather
        // than straight into a C identifier.
        Ty::Handle(name) => format!("Handle__{}", c_ident(name)),
        // A borrowed trait object is a struct per trait, so its mangled name is
        // the trait's, under its own prefix.
        Ty::Dyn { trait_name, .. } => format!("Dyn__{}", c_ident(trait_name)),
        Ty::BoxDyn { trait_name } => format!("BoxDyn__{}", c_ident(trait_name)),
        // A `Ptr<T>` type argument names the same C type `c_type` does, under
        // the mangling every other generated symbol uses.
        Ty::Ptr(elem) => format!("Ptr__{}", ty_mangle(elem)),
        // A function type never reaches a container or a type argument (both
        // are refused in `infer.rs`), but it is reached by `drop_fn_name`'s
        // unreachable arm and by diagnostics, so it gets an identifier-shaped
        // answer rather than a string with `(`/`)`/`:` in it.
        Ty::Fn { params, ret } => format!(
            "Fn__{}{}",
            params.iter().map(|p| format!("{}__", ty_mangle(p))).collect::<String>(),
            ty_mangle(ret)
        ),
        // A closure signature mangles the same way, under its own prefix, so
        // a program using both spellings of one signature gets two distinct C
        // types - which is the point: one is a pointer, the other owns an
        // environment.
        Ty::Closure { params, ret } => closure_mangle(params, ret),
        // A scoped signature is the same fat pointer, so it shares the
        // closure's C type rather than getting a second, identical typedef.
        Ty::ScopedFn { params, ret } => closure_mangle(params, ret),
        // Everything else is a scalar, `View`/`unit`, or a borrow. A scalar's
        // written form is already an identifier; a borrow's is `&T` or
        // `mut T`, and `&T` is spelled `refT` here. What that spelling could
        // *not* survive is a module-qualified inner type: `&json.Item` came
        // out `refjson.Item`, a name with a `.` in it, so the closure typedef
        // for `closure(&Item)` declared inside the `json` module itself was
        // `kelp_Closure__refjson.Item__unit` - not a C identifier at all, and
        // every generated use of it failed to compile. That is the shape a
        // library hits when it offers a callback over its own type, which is
        // why the fallback goes through `c_ident`, the one mangling every
        // other generated symbol uses, instead of replacing `&` by hand.
        other => c_ident(&other.display().replace('&', "ref")),
    }
}

/// The generated C struct tag for `List<T>`.
pub fn list_c_type(elem: &Ty) -> String {
    format!("kelp_List__{}", ty_mangle(elem))
}

/// The generated C struct tag for `Box<T>`.
pub fn box_c_type(elem: &Ty) -> String {
    format!("kelp_Box__{}", ty_mangle(elem))
}

/// The generated C struct tag for `Map<K, V>`.
pub fn map_c_type(key: &Ty, value: &Ty) -> String {
    format!("kelp_Map__{}__{}", ty_mangle(key), ty_mangle(value))
}

/// The generated C struct tag for `[T; N]` (A-18).
///
/// The storage is inline, so this is an ordinary struct wrapping a C array:
/// `typedef struct { T v[N]; } kelp_Array__T__N;`. The wrapper (rather than a
/// bare C array) is what makes the value obey Kelp's rules in C: assignment
/// copies it, a function can take and return it by value, and nothing decays
/// to a pointer behind the type system's back.
pub fn array_c_type(elem: &Ty, len: u64) -> String {
    format!("kelp_Array__{}__{}", ty_mangle(elem), len)
}

/// The generated C struct tag for `Atomic<T>`.
///
/// The storage is one plain machine word: `typedef struct { T v; } kelp_Atomic__T;`. Nothing marks the member `_Atomic` because every access is
/// written as an `__atomic_*` builtin (or, for the load, the same builtin)
/// rather than as a plain read or write, which is the property this type
/// exists to have. A plain member would not force anyone writing code by hand
/// to reach for an atomic operation, but nothing by hand writes this type:
/// every access is a call the two backends lower to one instruction.
pub fn atomic_c_type(elem: &Ty) -> String {
    format!("kelp_Atomic__{}", ty_mangle(elem))
}

/// The synthetic callee keys `infer.rs` lowers the two handle operations to.
/// A call's callee is a string that codegen matches, and these two never come
/// from a declaration, so they use a spelling no Kelp identifier can have.
/// `null` carries no argument and codegen reads the handle type off the call's
/// own `Ty::Handle`; `is_null` carries the handle as its one argument.
pub const HANDLE_NULL_KEY: &str = "$handle$null";
pub const HANDLE_IS_NULL_KEY: &str = "$handle$is_null";

/// The C struct tag for a `datac`, so a container can point at a struct
/// whose body has not been emitted yet (`Box<Node>` inside `Node` itself).
pub fn struct_tag(name: &str) -> String {
    format!("tag_{}", c_ident(name))
}

mod layout;
pub use layout::*;

/// The C identifier of a *fixed-arity* wrapper around a variadic user
/// `extern fun` (gap 7), from the extern's own key and the extra-argument
/// types one call passes. Deterministic, so two call sites that pass the same
/// types share one wrapper, and the typechecker and a backend cannot disagree
/// about which one a call names. The `c_name`/`symbol_name` prefix a backend
/// adds turns this into `kelp_va_<mangled fn>_<mangled types>`.
pub fn variadic_wrapper_name(extern_key: &str, extra: &[Ty]) -> String {
    let mut name = format!("va_{}", c_ident(extern_key));
    for ty in extra {
        name.push('_');
        name.push_str(&c_ident(&ty.display()));
    }
    name
}

/// Whether `ty` may be passed as an *extra* argument of a variadic user
/// `extern fun` (gap 7): an integer, `bool`, `f32`/`f64`, `str` or an opaque
/// handle. A `char` is not here: it is a code point, not an integer the
/// native side could read back from a `va_arg` of any fixed width. There is
/// no implicit conversion either way, so a type outside this set is a compile
/// error rather than a cast.
pub fn is_variadic_abi_ty(ty: &Ty) -> bool {
    ty.is_integer()
        || matches!(ty, Ty::Bool | Ty::F32 | Ty::F64 | Ty::Float | Ty::Str | Ty::Handle(_))
        || matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Str))
}

/// Mangles an `.kp` top-level name into a valid C identifier.
///
/// M5 prefixes every module's items with their module path
/// (`shapes.circle.Circle`), and a receiver method is keyed `Type#name` -
/// neither `.` nor `#` is legal in a C identifier, so both become `__`.
/// M6.3's stdlib overloads add `name@i32` keys, so `@` becomes `__ov_` and
/// the `,` between parameter types becomes `_`. A-2's instantiation keys add
/// `<`/`>` and the `, ` between type arguments (`Pair<i32, String>`), so `<>`
/// become `__l__`/`__g__` and the space is dropped. Not fully
/// collision-proof against a hand-written `shapes__circle__Circle` (nothing
/// can be, short of a real mangling scheme), but low-risk and simple, same
/// tradeoff as the pre-M5 `#`-only version.
///
/// BUG-zR8Wm: a module's id is its file stem, so a file named `we"ird.kp`
/// reaches here with the quote in it, and the capability array symbol the
/// backends spell from that name (`kelp_caps_we"ird_fs`) was not a C
/// identifier, so `cc` refused the whole build. Every byte the mappings above
/// leave alone is therefore outside `[A-Za-z0-9_]`, and the final sweep
/// spells each one as `__x` and its two hex digits: legal, fixed width, and
/// distinct per byte.
pub fn c_ident(name: &str) -> String {
    let mapped = name
        .replace('.', "__")
        .replace('#', "__")
        .replace('@', "__ov_")
        .replace(',', "_")
        .replace('<', "__l__")
        .replace('>', "__g__")
        // A function type argument (`Holder<fun(i32) => i32>`) is the one
        // type whose written form carries parentheses and a colon, and its
        // instantiation key is built from that written form, so the two have
        // to mangle too or the key is not a C identifier.
        .replace('(', "__lp__")
        .replace(')', "__rp__")
        .replace(':', "__c__")
        .replace(' ', "");
    if mapped.bytes().all(is_ident_byte) {
        return mapped;
    }
    let mut out = String::with_capacity(mapped.len() + 4);
    for b in mapped.bytes() {
        if is_ident_byte(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("__x{b:02x}"));
        }
    }
    out
}

/// Whether one byte may stand in a C identifier as itself.
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether `ty` is a *borrowed* type in the sense that matters for a `datac`
/// field or an `enum` payload: it points at data somebody else owns, so it
/// cannot be stored. (`Box`, `List` and `Map` are not reached here - they
/// *own* what they point at.)
///
/// One predicate rather than a `matches!` at each of the four sites that
/// enforce this, because there are four and a new borrowed type has to be
/// remembered at every one of them: the struct-table pass and the generic
/// instantiation path each check datac fields and enum payloads separately.
/// `View` (A-8) was missed at two of the four on the first attempt.
pub fn is_borrowed_field_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Str | Ty::View | Ty::Ref { .. } | Ty::Dyn { .. })
}

/// The C typedef name of the fat pair a borrowed trait object is: one struct per
/// trait, shared by `&dyn Name` and `&mut dyn Name` (the two are the same C
/// value; the borrow checker, not a C qualifier, tells them apart).
pub fn dyn_c_type(trait_name: &str) -> String {
    format!("kelp_Dyn__{}", c_ident(trait_name))
}

/// The emitted symbol of one `(trait, type)` vtable, the same name in both
/// backends so a diagnostic or a `nm` line reads the same either way.
pub fn dyn_vtable_symbol(trait_name: &str, type_name: &str) -> String {
    format!(
        "kelp_vtable__{}__{}",
        c_ident(trait_name),
        c_ident(type_name)
    )
}

/// The emitted symbol of the thunk one vtable slot points at.
pub fn dyn_thunk_symbol(trait_name: &str, type_name: &str, method: &str) -> String {
    format!(
        "kelp_dyn_thunk__{}__{}__{}",
        c_ident(trait_name),
        c_ident(type_name),
        c_ident(method)
    )
}

/// The emitted symbol of the thunk the vtable's drop entry points at (A-12
/// slice 7): the concrete type's own drop glue over the erased pointer.
pub fn dyn_drop_thunk_symbol(trait_name: &str, type_name: &str) -> String {
    format!(
        "kelp_dyn_drop__{}__{}",
        c_ident(trait_name),
        c_ident(type_name)
    )
}

/// The emitted symbol that frees an *owned* trait object (A-12 slice 7), one
/// per trait: `free` on the box's allocation, after the vtable's drop entry has
/// run the concrete type's own drop glue. The concrete type is not part of the
/// name, because the value behind the pointer is reached through the vtable.
pub fn box_dyn_drop_fn(trait_name: &str) -> String {
    format!("kelp_box_dyn_drop__{}", c_ident(trait_name))
}

/// A function's checked signature. Parameter names are kept (not just
/// types) because call sites can pass named arguments (`f(x = 1)`), which
/// `infer.rs` resolves against these names.
#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
}

/// One `datac` field: `val`/`var` name: Type.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    /// `var` field: reassignable (given a `var`-bound value). `val` field:
    /// never reassignable, regardless of the binding's own mutability.
    pub mutable: bool,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// The `repr(...)` clause, or `Repr::C` for the default C layout
    /// (kernel stage 2). It decides the field offsets and the type's own
    /// alignment, in this backend and in the C text it writes alike.
    pub repr: Repr,
    /// Position in the program's merged declaration order, across every
    /// `datac` *and* `enum` (A-1). A field or payload may only name an
    /// earlier declaration by value, so this order is a valid topological
    /// order  -  for `owned_types`' single pass and for the C typedefs
    /// the removed C backend emitted.
    pub order: usize,
}

/// One variant of an `enum` (A-1): a name and the payload fields it carries.
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// `Off = 0`: the explicit discriminant, already resolved to a value. A
    /// field-less enum's real tag value is [`EnumDef::discriminants`], which
    /// fills an unwritten discriminant in as the previous value plus one.
    pub discriminant: Option<i128>,
}

/// A tagged union (A-1): exactly one variant is live in a value. At the type
/// level an `enum` is a `Ty::Struct`, so it is nominal, non-`Copy` and
/// droppable exactly like a `datac`; the difference lives here, in the
/// declaration table, in the pattern checker and in the C layout
/// (`tag` + `union` instead of a flat field list).
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<VariantDef>,
    /// The `repr(...)` clause, or `Repr::C` for the default C layout and an
    /// `i32` tag (kernel stage 2). Only `repr(<int>)` changes an enum's
    /// layout: it picks the tag's storage type.
    pub repr: Repr,
    /// See `StructDef::order`.
    pub order: usize,
}

impl EnumDef {
    pub fn variant(&self, name: &str) -> Option<(usize, &VariantDef)> {
        self.variants.iter().enumerate().find(|(_, v)| v.name == name)
    }

    /// Whether every variant carries no payload. Only a field-less enum may
    /// spell explicit discriminants and take a `repr(<int>)` tag type, so this
    /// is the gate for both.
    pub fn is_field_less(&self) -> bool {
        self.variants.iter().all(|v| v.is_payload_less())
    }

    /// The tag's storage, as `(bytes, signed)`, taken from the `repr(<int>)`
    /// name. It is the width the C typedef and the LLVM struct both use, and
    /// it is deliberately not a `Ty`: v1 has no `i8`/`i16` *value* type, but
    /// `repr(i8)`/`repr(i16)` still name a one- or two-byte tag.
    pub fn tag_storage(&self) -> (u64, bool) {
        match &self.repr {
            Repr::Int(name) => match name.as_str() {
                "u8" => (1, false),
                "u16" => (2, false),
                "u32" => (4, false),
                "u64" => (8, false),
                "i8" => (1, true),
                "i16" => (2, true),
                "i64" => (8, true),
                _ => (4, true),
            },
            _ => (4, true),
        }
    }

    /// The integer `Ty` a tag value crosses the surface as: the narrowest
    /// Kelp integer that holds the storage. Kelp has no `i8`/`i16`, so those
    /// two spellings report `i32`; every other one reports itself.
    pub fn tag_ty(&self) -> Ty {
        match &self.repr {
            Repr::Int(name) => match name.as_str() {
                "u8" => Ty::U8,
                "u16" => Ty::U16,
                "u32" => Ty::U32,
                "u64" => Ty::U64,
                "i64" => Ty::I64,
                _ => Ty::I32,
            },
            _ => Ty::I32,
        }
    }

    /// The tag value of every variant, in declaration order: a written
    /// discriminant stands, and an unwritten one is the previous value plus
    /// one, starting at 0 (kernel stage 2).
    pub fn discriminants(&self) -> Vec<i128> {
        let mut out = Vec::with_capacity(self.variants.len());
        let mut next: i128 = 0;
        for v in &self.variants {
            let value = v.discriminant.unwrap_or(next);
            out.push(value);
            next = value.saturating_add(1);
        }
        out
    }
}

impl VariantDef {
    /// The fields this variant actually stores: every declared field except
    /// one typed `unit`, which carries no runtime representation
    /// (`MISSING-r7Wqb`). `Ok`/`Some` instantiated with a `unit` type
    /// argument - the shape `thread.join` builds for a `Thread<unit>` - is
    /// declared with one field, and that field stores nothing.
    pub fn real_fields(&self) -> impl Iterator<Item = &FieldDef> {
        self.fields.iter().filter(|f| f.ty != Ty::Unit)
    }

    /// Whether this variant's union member is the bare placeholder byte
    /// (`char kelp_none` in C, `i8` in LLVM) rather than a payload struct: no
    /// fields at all, or every field `unit`. Both backends lay out, construct,
    /// read and derive over a variant by this rule rather than by
    /// `fields.is_empty()`, so a `unit` payload is exactly as payload-less as
    /// no payload.
    pub fn is_payload_less(&self) -> bool {
        self.real_fields().next().is_none()
    }
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

#[cfg(test)]
#[path = "types/tests.rs"]
mod tests;

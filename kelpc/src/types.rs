#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    I32,
    I64,
    U32,
    U64,
    F32,
    F64,
    Bool,
    Char,
    Unit,
    Str,
    /// `u8`/`u16`: byte-level integers, and `usize`/`isize`: the index and
    /// size types `Bytes` is measured and indexed with. Added in M6 because
    /// a byte buffer cannot be described without them.
    U8,
    U16,
    Usize,
    Isize,
    /// The builtin owned heap byte buffer (M6): a pointer plus a length,
    /// allocated on construction and freed at the end of the owning scope.
    /// Non-`Copy`, so it moves; `&Bytes` is the borrow (`Ty::Ref`).
    Bytes,
    /// The builtin owned heap utf-8 text type (M6.2). Same representation as
    /// `Bytes` (pointer plus length) and the same move/drop rules; it exists
    /// as its own type so text and arbitrary bytes are not interchangeable.
    /// `String.from_bytes`/`String.into_bytes` convert with an ownership
    /// transfer, not a cast.
    String,
    /// A `datac` value type, named by its declaration.
    Struct(String),
    /// The builtin owned growable sequence (collections milestone):
    /// `List<T>`, where `T` is a scalar, `String`, `Bytes`, or a `datac`.
    /// Non-`Copy`: assigning or passing one moves it, and it is freed at the
    /// end of the owning scope — its elements first, then its buffer. The
    /// element type is part of the type, and codegen monomorphizes: one C
    /// struct plus one set of functions per element type actually used (see
    /// `codegen_c.rs::write_list`).
    List(Box<Ty>),
    /// The builtin owning pointer (M8): `Box<T>` holds one `T` on the heap,
    /// or nothing at all (`Box.empty()`), which is what makes a recursive
    /// `datac` expressible — `datac Node(val next: Box<Node>)` would be
    /// infinitely sized by value, and a pointer is the only way out.
    /// Non-`Copy`; dropping it drops the value it holds and frees the box.
    /// Like `List`, monomorphized: one C struct plus one function set per
    /// element type actually used.
    Box(Box<Ty>),
    /// The builtin owned keyed container (MK5-S5): `Map<K, V>` — a key type
    /// and a value type, both monomorphized. `Map<V>` is the shorthand for a
    /// `String` key, which is what every pre-A-2 use site writes, and the two
    /// spellings are the same `Ty`. A key must be hashable and comparable by
    /// the compiler (`is_map_key`): `String` or a scalar. Non-`Copy`; a map
    /// owns its keys and its values, and drops both at the end of the owning
    /// scope. The value type is part of the type, so `Map<String, i64>` and
    /// `Map<i32, i64>` are different C types with different functions.
    /// `Map<String, Map<i32>>` is as impossible as `List<List<i32>>`, by the
    /// same element check.
    Map(Box<Ty>, Box<Ty>),
    /// A borrow of a value of type `inner` — shared (`mutable: false`) or
    /// exclusive (`mutable: true`). Never appears as a function's declared
    /// return type in M4 (no surface syntax produces one there — see
    /// `infer::resolve_type`), so a borrow can never escape the call frame
    /// that created it; that's what makes the borrow checker's call-scoped
    /// release sound without real lifetime tracking. A reference is always
    /// Copy regardless of `inner` — copying a pointer doesn't move what it
    /// points at.
    Ref { mutable: bool, inner: Box<Ty> },
    /// A function value (A-4): a plain C function pointer, with no captured
    /// environment — so it is `Copy`, it owns nothing, and it needs no drop.
    /// `params` are the *declared* parameter types, already through the
    /// parameter rule (an owned type is a borrow unless the type was written
    /// `move T`), which is what makes a written function type and a
    /// declaration's signature the same type when they spell the same thing.
    ///
    /// A bare name of a declared `fun` is how a value of this type is made;
    /// calling one goes through `tir::ExprKind::IndirectCall`. A `fun`
    /// literal that captured nothing is the same value; one that *captured*
    /// something is a `Ty::Closure` below, which carries the environment it
    /// captured. This variant is deliberately the environment-free half: a
    /// plain C function pointer, `Copy` (A-4 phases B/C).
    Fn { params: Vec<Ty>, ret: Box<Ty> },
    /// A function value that carries an environment (A-4 phase E): what a
    /// `fun` literal whose body names a binding from the enclosing scope
    /// becomes. Written `closure(T).R: U` — the same shape as a function
    /// type, under the `closure` keyword, because it is called the same way.
    ///
    /// The representation differs from `Ty::Fn` in exactly the ways that
    /// matter to every pass: a closure is `{ fn, env, drop }` — a *fat*
    /// pointer, monomorphized per signature — so it is **not `Copy`**, it
    /// owns the heap environment the captures were moved into, and it drops
    /// that environment when it dies. A `Ty::Fn` stays a plain C function
    /// pointer: a declared function, and a literal that captured nothing,
    /// have no environment to carry (see `codegen_c.rs::write_closure`).
    ///
    /// Calling one goes through `tir::ExprKind::IndirectCall`, exactly as a
    /// `Ty::Fn` does; codegen is what knows a closure's call spelling
    /// (`c.fn(c.env, ...)`). A capturing literal is the only way a value of
    /// this type is made, so a program cannot build a closure without
    /// something to capture.
    Closure { params: Vec<Ty>, ret: Box<Ty> },
}

impl Ty {
    pub fn from_name(name: &str) -> Option<Ty> {
        Some(match name {
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "f32" => Ty::F32,
            "f64" => Ty::F64,
            "bool" => Ty::Bool,
            "char" => Ty::Char,
            "unit" => Ty::Unit,
            "str" => Ty::Str,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "usize" => Ty::Usize,
            "isize" => Ty::Isize,
            "Bytes" => Ty::Bytes,
            "String" => Ty::String,
            _ => return None,
        })
    }

    /// Whether a value of this type is `Copy`. Everything is, except the
    /// kinds of thing that own heap memory: a `datac` and the builtin
    /// `Bytes`/`String`. Reading a non-`Copy` value as a whole moves it (M3).
    pub fn is_copy(&self) -> bool {
        !matches!(
            self,
            Ty::Struct(_)
                | Ty::Bytes
                | Ty::String
                | Ty::List(_)
                | Ty::Box(_)
                | Ty::Map(..)
                | Ty::Closure { .. }
        )
    }

    /// Whether this type *itself* owns heap memory, ignoring whether a
    /// `datac` does. Use `needs_drop` below when the struct table is
    /// available — a `datac` with an owned field owns memory too, and out of
    /// context `Ty::Struct(_)` cannot know that.
    pub fn owns_heap(&self) -> bool {
        matches!(
            self,
            Ty::Bytes | Ty::String | Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Closure { .. }
        )
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Ty::I32
                | Ty::I64
                | Ty::U32
                | Ty::U64
                | Ty::F32
                | Ty::F64
                | Ty::U8
                | Ty::U16
                | Ty::Usize
                | Ty::Isize
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Ty::I32 | Ty::I64 | Ty::U32 | Ty::U64 | Ty::U8 | Ty::U16 | Ty::Usize | Ty::Isize
        )
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }

    pub fn c_type(&self) -> String {
        match self {
            Ty::I32 => "int32_t".to_string(),
            Ty::I64 => "int64_t".to_string(),
            Ty::U32 => "uint32_t".to_string(),
            Ty::U64 => "uint64_t".to_string(),
            Ty::F32 => "float".to_string(),
            Ty::F64 => "double".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Char => "char".to_string(),
            Ty::Unit => "void".to_string(),
            Ty::Str => "const char*".to_string(),
            Ty::U8 => "uint8_t".to_string(),
            Ty::U16 => "uint16_t".to_string(),
            Ty::Usize => "size_t".to_string(),
            Ty::Isize => "ptrdiff_t".to_string(),
            Ty::Bytes => "kelp_Bytes".to_string(),
            Ty::String => "kelp_String".to_string(),
            Ty::List(elem) => list_c_type(elem),
            Ty::Box(elem) => box_c_type(elem),
            Ty::Map(key, value) => map_c_type(key, value),
            // A `&str` is the same C value as the `str` it points at (both
            // `const char*`): a reference to an already-borrowed view is not
            // a pointer to a pointer. The borrow lives in the type system
            // and the borrow checker, not in the representation.
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => "const char*".to_string(),
            // A function *type* is a pointer type, and a pointer type cannot
            // be printed as `ret (*)(params)` and then followed by a name
            // (`c_decl` is what puts a declarator together). Inside another
            // declarator this spelling is the correct C fragment, which is
            // all a return position needs.
            Ty::Fn { params, ret } => {
                format!("{} (*)({})", ret.c_type(), fn_params_c(params))
            }
            // A closure is an ordinary C struct (a fat pointer), so it is
            // named by its typedef and never needs the declarator trick
            // above — including in a return position, which is why a
            // function may return a closure but not a bare function value.
            Ty::Closure { params, ret } => closure_c_type(params, ret),
            Ty::Struct(name) => format!("struct_{}", c_ident(name)),
            Ty::Ref { mutable, inner } => {
                if *mutable {
                    format!("{}*", inner.c_type())
                } else {
                    format!("const {}*", inner.c_type())
                }
            }
        }
    }

    pub fn display(&self) -> String {
        match self {
            Ty::I32 => "i32".to_string(),
            Ty::I64 => "i64".to_string(),
            Ty::U32 => "u32".to_string(),
            Ty::U64 => "u64".to_string(),
            Ty::F32 => "f32".to_string(),
            Ty::F64 => "f64".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Char => "char".to_string(),
            Ty::Unit => "unit".to_string(),
            Ty::Str => "str".to_string(),
            Ty::U8 => "u8".to_string(),
            Ty::U16 => "u16".to_string(),
            Ty::Usize => "usize".to_string(),
            Ty::Isize => "isize".to_string(),
            Ty::Bytes => "Bytes".to_string(),
            Ty::String => "String".to_string(),
            Ty::Struct(name) => name.clone(),
            Ty::List(elem) => format!("List<{}>", elem.display()),
            Ty::Box(elem) => format!("Box<{}>", elem.display()),
            Ty::Map(key, value) => format!("Map<{}, {}>", key.display(), value.display()),
            Ty::Ref { mutable: true, inner } => format!("mut {}", inner.display()),
            Ty::Ref { mutable: false, inner } => format!("&{}", inner.display()),
            Ty::Fn { params, ret } => {
                // The return is written `.R: T` exactly as a declaration
                // writes it, and a `Unit` return is left off — so a written
                // function type and a signature read the same way.
                let mut s = format!("fun({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(".R: {}", ret.display()));
                }
                s
            }
            Ty::Closure { params, ret } => {
                // The closure type reads exactly as the function type does,
                // one keyword apart, because the two are called identically
                // and differ only in what the value owns.
                let mut s = format!("closure({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(".R: {}", ret.display()));
                }
                s
            }
        }
    }

    /// The C declaration of a variable called `name` with this type: the C
    /// type followed by the name, except for a function value, where the name
    /// goes *inside* the pointer declarator (`int32_t (*kelp_f)(int32_t)`).
    /// Every declaration site — a parameter, a `val`/`var`, a `datac` field,
    /// an enum payload, a passdown — goes through this rather than pairing
    /// `c_type()` with a name by hand, because a function type is the one type
    /// where that pairing does not produce valid C.
    pub fn c_decl(&self, name: &str) -> String {
        match self {
            Ty::Fn { params, ret } => {
                format!("{} (*{name})({})", ret.c_type(), fn_params_c(params))
            }
            other => format!("{} {name}", other.c_type()),
        }
    }
}

/// `ret (*)(params)`'s parameter half: `void` for none, the C parameter types
/// otherwise. Shared with `c_decl`, since a declarator and a bare function
/// pointer type differ only in where the name goes.
fn fn_params_c(params: &[Ty]) -> String {
    if params.is_empty() {
        "void".to_string()
    } else {
        params.iter().map(|p| p.c_type()).collect::<Vec<_>>().join(", ")
    }
}

/// `T, U` for a diagnostic or a written type.
fn join_types(types: &[Ty]) -> String {
    types.iter().map(|t| t.display()).collect::<Vec<_>>().join(", ")
}

/// The mangled signature fragment a closure instantiation is named by:
/// `i32__i32` is `closure(i32).R: i32`. Parameters first, in order, then the
/// return — the same order `ty_mangle`'s function-type arm uses, under a
/// different prefix.
pub fn closure_mangle(params: &[Ty], ret: &Ty) -> String {
    format!(
        "Closure__{}{}",
        params.iter().map(|p| format!("{}__", ty_mangle(p))).collect::<String>(),
        ty_mangle(ret)
    )
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
/// and `codegen_c.rs` emits the reads.
pub fn capture_field(index: usize) -> String {
    format!("c{index}")
}

/// The generated C struct tag for a literal's environment, and the function
/// that frees it (A-4 phase E). One place names both, because `infer.rs`
/// records them on the generated function and `codegen_c.rs` emits them.
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
/// so one pass in merged declaration order is already a valid fixpoint — no
/// repeating until stable. Merged order matters here for the first time in
/// A-1: a `datac` and an `enum` may each name the other's earlier-declared
/// half, so the two declaration lists have to be walked as one sequence.
pub fn owned_types(
    structs: &[StructDef],
    enums: &[EnumDef],
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

    let mut owned = std::collections::HashSet::new();
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
/// byte-wise (length-aware, not `strcmp` — an embedded NUL is possible). A
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

/// Whether a `List<T>` element can be copied out of the list, which is what
/// `List.slice` does: a scalar is copied, `String`/`Bytes` are cloned (their
/// own buffer), and a `datac` cannot be (v1 has no deep copy, and a shallow
/// one would be a second owner).
pub fn list_clonable_elem(ty: &Ty) -> bool {
    ty.is_copy() || matches!(ty, Ty::String | Ty::Bytes)
}

/// Whether a value of `ty` has to be freed — the builtin owned types, or a
/// `datac` that (transitively) holds one.
pub fn needs_drop(ty: &Ty, owned_structs: &std::collections::HashSet<String>) -> bool {
    match ty {
        Ty::Struct(name) => owned_structs.contains(name),
        other => other.owns_heap(),
    }
}

/// The C function that frees a value of `ty`, taking a pointer to it.
/// Generated per `datac` by `codegen_c.rs`; the builtins are in the fixed
/// runtime preamble.
pub fn drop_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Bytes => "kelp_bytes_drop".to_string(),
        Ty::String => "kelp_string_drop".to_string(),
        Ty::Struct(name) => format!("kelp_drop_{}", c_ident(name)),
        Ty::List(elem) => format!("kelp_list_drop__{}", ty_mangle(elem)),
        Ty::Box(elem) => format!("kelp_box_drop__{}", ty_mangle(elem)),
        Ty::Map(key, value) => {
            format!("kelp_map_drop__{}__{}", ty_mangle(key), ty_mangle(value))
        }
        Ty::Closure { params, ret } => {
            format!("kelp_closure_drop__{}", closure_mangle(params, ret))
        }
        other => unreachable!("no drop rule for type '{}'", other.display()),
    }
}

/// Whether `ty` may be a `List<T>` element. A borrow cannot be one (an
/// element outlives the borrow that produced it, and v1 has no lifetime
/// syntax to say otherwise), and neither can `str` (the same borrowed view)
/// or `unit`. Everything else — the scalars, `String`, `Bytes`, and a
/// `datac` — has a plain C representation that a growable array can hold.
/// A `Box<T>` is *not* one: keeping list elements pointer-free is what lets
/// the list's C struct be emitted before the `datac`s (see
/// `codegen_c.rs`), and a `List<Box<T>>` has no use a `Box<List<T>>` or a
/// plain `List<T>` does not cover.
pub fn is_list_element(ty: &Ty) -> bool {
    match ty {
        Ty::Bytes | Ty::String | Ty::Struct(_) => true,
        Ty::Ref { .. } | Ty::Str | Ty::Unit | Ty::List(_) | Ty::Box(_) | Ty::Map(..) => false,
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// Whether `ty` may be a `Map<K, V>` value. The same set `List<T>` accepts: a
/// scalar, `String`, `Bytes`, or a `datac`. A map is neither a list element
/// nor a map value, so `Map<String, Map<i32>>` and `Map<String, List<i32>>`
/// stay impossible.
pub fn is_map_element(ty: &Ty) -> bool {
    is_list_element(ty)
}

/// Whether `ty` may be a `Map<K, V>` key. The compiler generates the key's
/// hash and its equality, so the set is closed: `String` (byte-wise, with a
/// length so an embedded NUL is safe), the integers, and `bool`. A float is
/// refused because `nan != nan` would make a key unfindable, and a `datac`,
/// a container or a borrow is refused because v1 has no equality for them —
/// a generic key would need a trait or a comparator function, neither of
/// which exists (A-12, A-4).
pub fn is_map_key(ty: &Ty) -> bool {
    matches!(ty, Ty::String | Ty::Bool) || ty.is_integer()
}

/// Whether `ty` may be a `Box<T>` element. A box holds a *pointer*, so the
/// element may be anything with a size — including another container and,
/// crucially, the `datac` the box is a field of (that is how a recursive
/// type is written). A borrow or a `str` view is still refused: a box
/// outlives the borrow it was made from, and there is nowhere to put that
/// lifetime.
pub fn is_box_element(ty: &Ty) -> bool {
    match ty {
        // A closure is an ordinary C struct, and it is emitted before every
        // `Box` instantiation that names one (A-4 phase E), so a box is the
        // one container that can hold a closure — `Box<closure().R: i32>` is
        // how an *optional* callback is written (`Box.empty()`).
        Ty::Bytes | Ty::String | Ty::Struct(_) | Ty::List(_) | Ty::Box(_) | Ty::Closure { .. } => {
            true
        }
        Ty::Ref { .. } | Ty::Str | Ty::Unit => false,
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

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
        // types — which is the point: one is a pointer, the other owns an
        // environment.
        Ty::Closure { params, ret } => closure_mangle(params, ret),
        other => other.display().replace('&', "ref"),
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

/// The `List<T>` or `Box<T>` behind an expression's type, looking through a
/// borrow — `l.push(x)` and `b.get()` both arrive with the receiver typed
/// `&List<T>`/`&Box<T>` (a container parameter is a borrow, and a method
/// receiver is auto-borrowed).
pub fn list_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::List(elem) => Some(elem),
        Ty::Ref { inner, .. } => list_elem_of(inner),
        _ => None,
    }
}

/// As `list_elem_of`, for `Box<T>`.
pub fn box_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Box(elem) => Some(elem),
        Ty::Ref { inner, .. } => box_elem_of(inner),
        _ => None,
    }
}

/// As `list_elem_of`, for `Map<K, V>` — the *value* type. The key is what the
/// call-site argument has to match, and everything else about a map method
/// (`get`, `insert`, `remove`, …) is keyed on the value.
pub fn map_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Map(_, value) => Some(value),
        Ty::Ref { inner, .. } => map_elem_of(inner),
        _ => None,
    }
}

/// As `map_elem_of`, for the *key* type.
pub fn map_key_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Map(key, _) => Some(key),
        Ty::Ref { inner, .. } => map_key_of(inner),
        _ => None,
    }
}

/// The C spelling of an element type *inside a container's own typedef*.
/// A container is emitted before the `datac`s (its buffer is a pointer), so
/// a `datac` element is named by its forward-declared struct tag rather than
/// by its typedef name; every other element is named as usual.
pub fn container_elem_c_type(elem: &Ty) -> String {
    match elem {
        Ty::Struct(name) => format!("struct tag_{}", c_ident(name)),
        other => other.c_type(),
    }
}

/// The C struct tag for a `datac`, so a container can point at a struct
/// whose body has not been emitted yet (`Box<Node>` inside `Node` itself).
pub fn struct_tag(name: &str) -> String {
    format!("tag_{}", c_ident(name))
}

/// Mangles an `.kp` top-level name into a valid C identifier.
///
/// M5 prefixes every module's items with their module path
/// (`shapes.circle.Circle`), and a receiver method is keyed `Type#name` —
/// neither `.` nor `#` is legal in a C identifier, so both become `__`.
/// M6.3's stdlib overloads add `name@i32` keys, so `@` becomes `__ov_` and
/// the `,` between parameter types becomes `_`. A-2's instantiation keys add
/// `<`/`>` and the `, ` between type arguments (`Pair<i32, String>`), so `<>`
/// become `__l__`/`__g__` and the space is dropped. Not fully
/// collision-proof against a hand-written `shapes__circle__Circle` (nothing
/// can be, short of a real mangling scheme), but low-risk and simple, same
/// tradeoff as the pre-M5 `#`-only version.
pub fn c_ident(name: &str) -> String {
    name.replace('.', "__")
        .replace('#', "__")
        .replace('@', "__ov_")
        .replace(',', "_")
        .replace('<', "__l__")
        .replace('>', "__g__")
        // A function type argument (`Holder<fun(i32).R: i32>`) is the one
        // type whose written form carries parentheses and a colon, and its
        // instantiation key is built from that written form, so the two have
        // to mangle too or the key is not a C identifier.
        .replace('(', "__lp__")
        .replace(')', "__rp__")
        .replace(':', "__c__")
        .replace(' ', "")
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
    /// Position in the program's merged declaration order, across every
    /// `datac` *and* `enum` (A-1). A field or payload may only name an
    /// earlier declaration by value, so this order is a valid topological
    /// order — for `owned_types`' single pass and for the C typedefs
    /// `codegen_c.rs` emits.
    pub order: usize,
}

/// One variant of an `enum` (A-1): a name and the payload fields it carries.
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
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
    /// See `StructDef::order`.
    pub order: usize,
}

impl EnumDef {
    pub fn variant(&self, name: &str) -> Option<(usize, &VariantDef)> {
        self.variants.iter().enumerate().find(|(_, v)| v.name == name)
    }
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

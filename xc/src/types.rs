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
    /// A borrow of a value of type `inner` — shared (`mutable: false`) or
    /// exclusive (`mutable: true`). Never appears as a function's declared
    /// return type in M4 (no surface syntax produces one there — see
    /// `infer::resolve_type`), so a borrow can never escape the call frame
    /// that created it; that's what makes the borrow checker's call-scoped
    /// release sound without real lifetime tracking. A reference is always
    /// Copy regardless of `inner` — copying a pointer doesn't move what it
    /// points at.
    Ref { mutable: bool, inner: Box<Ty> },
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
        !matches!(self, Ty::Struct(_) | Ty::Bytes | Ty::String | Ty::List(_))
    }

    /// Whether this type *itself* owns heap memory, ignoring whether a
    /// `datac` does. Use `needs_drop` below when the struct table is
    /// available — a `datac` with an owned field owns memory too, and out of
    /// context `Ty::Struct(_)` cannot know that.
    pub fn owns_heap(&self) -> bool {
        matches!(self, Ty::Bytes | Ty::String | Ty::List(_))
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
            Ty::Bytes => "x_Bytes".to_string(),
            Ty::String => "x_String".to_string(),
            Ty::List(elem) => list_c_type(elem),
            // A `&str` is the same C value as the `str` it points at (both
            // `const char*`): a reference to an already-borrowed view is not
            // a pointer to a pointer. The borrow lives in the type system
            // and the borrow checker, not in the representation.
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => "const char*".to_string(),
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
            Ty::Ref { mutable: true, inner } => format!("mut {}", inner.display()),
            Ty::Ref { mutable: false, inner } => format!("&{}", inner.display()),
        }
    }
}

/// Which `datac`s own heap memory, transitively. A `datac` may only name
/// `datac`s declared before it (open item 12), so one pass in declaration
/// order is already a valid fixpoint — no repeating until stable.
pub fn owned_structs(structs: &[StructDef]) -> std::collections::HashSet<String> {
    let mut owned = std::collections::HashSet::new();
    for s in structs {
        if s.fields.iter().any(|f| needs_drop(&f.ty, &owned)) {
            owned.insert(s.name.clone());
        }
    }
    owned
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
        Ty::Bytes => "x_bytes_drop".to_string(),
        Ty::String => "x_string_drop".to_string(),
        Ty::Struct(name) => format!("x_drop_{}", c_ident(name)),
        Ty::List(elem) => format!("x_list_drop__{}", list_elem_ident(elem)),
        other => unreachable!("no drop rule for type '{}'", other.display()),
    }
}

/// Whether `ty` may be a `List<T>` element. A borrow cannot be one (an
/// element outlives the borrow that produced it, and v1 has no lifetime
/// syntax to say otherwise), and neither can `str` (the same borrowed view)
/// or `unit`. Everything else — the scalars, `String`, `Bytes`, and a
/// `datac` — has a plain C representation that a growable array can hold.
pub fn is_list_element(ty: &Ty) -> bool {
    match ty {
        Ty::Bytes | Ty::String | Ty::Struct(_) => true,
        Ty::Ref { .. } | Ty::Str | Ty::Unit => false,
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// The C name fragment for a `List<T>`'s element type: `List<i32>` becomes
/// `x_List__i32`, `List<shapes.circle.Circle>` becomes
/// `x_List__shapes__circle__Circle` (through the same `c_ident` mangling
/// every other generated symbol uses).
pub fn list_elem_ident(elem: &Ty) -> String {
    match elem {
        Ty::Struct(name) => c_ident(name),
        Ty::Bytes => "Bytes".to_string(),
        Ty::String => "String".to_string(),
        other => other.display().replace('&', "ref"),
    }
}

/// The generated C struct tag for `List<T>`.
pub fn list_c_type(elem: &Ty) -> String {
    format!("x_List__{}", list_elem_ident(elem))
}

/// The `List<T>` behind an expression's type, looking through a borrow —
/// `l.push(x)` and `l.len()` both arrive with the receiver typed `&List<T>`
/// (a `List` parameter is a borrow, and a method receiver is auto-borrowed).
pub fn list_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::List(elem) => Some(elem),
        Ty::Ref { inner, .. } => list_elem_of(inner),
        _ => None,
    }
}

/// Mangles an `.x` top-level name into a valid C identifier.
///
/// M5 prefixes every module's items with their module path
/// (`shapes.circle.Circle`), and a receiver method is keyed `Type#name` —
/// neither `.` nor `#` is legal in a C identifier, so both become `__`.
/// M6.3's stdlib overloads add `name@i32` keys, so `@` becomes `__ov_` and
/// the `,` between parameter types becomes `_`. Not fully collision-proof
/// against a hand-written `shapes__circle__Circle` (nothing can be, short of
/// a real mangling scheme), but low-risk and simple, same tradeoff as the
/// pre-M5 `#`-only version.
pub fn c_ident(name: &str) -> String {
    name.replace('.', "__")
        .replace('#', "__")
        .replace('@', "__ov_")
        .replace(',', "_")
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
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

use super::*;

/// Whether `ty` may be a `List<T>` element. A borrow cannot be one (an
/// element outlives the borrow that produced it, and v1 has no lifetime
/// syntax to say otherwise), and neither can `str` (the same borrowed view),
/// `View`, `unit`, a function value, an atomic, or a `Map`.
///
/// A `List<T>` and a `Box<T>` *are* elements (A-10 slice 1), but only once
/// the container's own element type is decided: an element lives in the list's
/// own buffer, so the element type has to be a complete, droppable C type with
/// a name, and a container that still carries the open-container marker has
/// neither. `container_depth` in the removed C backend emitted a nested container
/// before the one that contains it, so every typedef and every function the
/// outer list calls already exists. A `Map` stays out because a map value is
/// not a container either.
pub fn is_list_element(ty: &Ty) -> bool {
    match ty {
        Ty::List(_) | Ty::Box(_) => !contains_open_container(ty),
        // An owned trait object is a complete two-word struct with its own
        // drop, so a list can hold one and free it element by element.
        Ty::BoxDyn { .. } => true,
        Ty::Bytes | Ty::String | Ty::Struct(_) => true,
        // A handle is pointer-sized and `Copy`, so it lives in a list's
        // buffer as a pointer and needs no element drop, exactly as a scalar
        // does.
        Ty::Handle(_) => true,
        Ty::Ref { .. } | Ty::Str | Ty::View | Ty::Unit | Ty::Map(..) => false,
        // A raw pointer is pointer-sized and `Copy`, so it lives inline in a
        // list's buffer and needs no element drop, exactly as a handle does.
        Ty::Ptr(_) => true,
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// Whether `ty` is a container that still carries the open-container marker in
/// its element slot, or a `List`/`Box` that holds such a container inside it.
/// This walks `List`/`Box` elements only, because those are the two kinds that
/// were widened to a container (A-10 slice 1) and a `Map` value is never an
/// element.
///
/// The marker is `Unit` where an element type goes (see `is_open_container`),
/// and no program wrote it: the generated typedef, the `push` prototype and the
/// drop all name the element type, so a container that still carries the marker
/// cannot be an element. `is_list_element` and `is_box_element` therefore
/// refuse it at any depth: an outer container naming an inner one would only
/// carry the marker further in.
pub fn contains_open_container(ty: &Ty) -> bool {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => **elem == Ty::Unit || contains_open_container(elem),
        _ => false,
    }
}

/// Whether `ty` may be a `Map<K, V>` value: a scalar, `String`, `Bytes`, or a
/// `datac`. Narrower than the `List` element set, which also takes a `List` or
/// a `Box` (A-10 slice 1): a map value sits inline in the entry but is read and
/// written as a plain location by the map's own routines, so a container value
/// is left out for now. `Map<String, Map<i32>>` and `Map<String, List<i32>>`
/// stay impossible.
pub fn is_map_element(ty: &Ty) -> bool {
    is_plain_element(ty)
}

/// The element set a `Map` value and an array element share, and the set the
/// `List` element check held before A-10 slice 1 widened it: a scalar,
/// `String`, `Bytes`, or a `datac`. Both stores read and write an element as a
/// plain location, so a container, a borrow or a view, an atomic, a function
/// value, and `unit` are all out.
pub(super) fn is_plain_element(ty: &Ty) -> bool {
    match ty {
        Ty::Bytes | Ty::String | Ty::Struct(_) => true,
        // A pointer is one machine word, so it is a plain element on the same
        // footing a handle is.
        Ty::Ptr(_) => true,
        Ty::Ref { .. } | Ty::Str | Ty::View | Ty::Unit | Ty::List(_) | Ty::Box(_) | Ty::Map(..) => {
            false
        }
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// Whether `ty` may be an array element or an array itself: `[T; N]` holds its
/// elements inline, so the element set is a scalar, `String`, `Bytes`, a
/// `datac`, or another array, so a nested `[[u8; 4]; 8]` is a page table and is
/// as expressible as it is in C. A `List`/`Box`/`Map` element stays refused
/// even now that a `List` element may be one: an array element is held inline,
/// so `[[u8; 4]; 8]` stays a fixed buffer, while a container element would make
/// the array a variable-size thing it is not. A borrow is refused for the
/// reason it always is: an element outlives the borrow it came from.
pub fn is_array_element(ty: &Ty) -> bool {
    match ty {
        Ty::Array(elem, _) => is_array_element(elem),
        other => is_plain_element(other),
    }
}

/// Whether `ty` names a key the compiler itself can hash and compare, with no
/// declaration graph consulted: `String` (byte-wise, with a length so an
/// embedded NUL is safe), the integers, `bool`, and `char`. A float is refused
/// because `nan != nan` would make an inserted key unfindable, and a container
/// or a borrow is refused because neither has a hash and an equality the
/// compiler can derive.
///
/// A `datac`/`enum` key is not answered here: whether one is keyable depends on
/// its fields, so a caller that has the declaration graph uses
/// `is_map_key_with` instead. This is the builtin half, and `is_map_key_with`
/// is the two of them together.
pub fn is_map_key(ty: &Ty) -> bool {
    matches!(ty, Ty::String | Ty::Bool | Ty::Char) || ty.is_integer()
}

/// Whether `ty` may be a `Map<K, V>` key, given the user types a key may be
/// (`map_key_types`): the builtin key set, or a `datac`/`enum` that is keyable
/// by derivation or that declares its own `hash`/`eq` pair (A-12). The key's
/// hash and its equality are whichever pair the type has, so nothing here
/// needs a trait or a comparator; a type outside the set is refused.
pub fn is_map_key_with(ty: &Ty, keyable: &std::collections::HashSet<String>) -> bool {
    value_is_keyable(ty, keyable)
}

/// Whether `ty` may be a `Box<T>` element. A box holds a *pointer*, so the
/// element may be anything with a size, including another container (as long
/// as that container's own element type is decided, see
/// `contains_open_container`) and, crucially, the `datac` the box is a field
/// of (that is how a recursive type is written). A borrow or a `str` view is
/// still refused: a box outlives the borrow it was made from, and there is
/// nowhere to put that lifetime.
pub fn is_box_element(ty: &Ty) -> bool {
    match ty {
        // A closure is an ordinary C struct, and it is emitted before every
        // `Box` instantiation that names one (A-4 phase E), so a box is the
        // one container that can hold a closure - `Box<closure fun() => i32>` is
        // how an *optional* callback is written (`Box.empty()`).
        Ty::Bytes
        | Ty::String
        | Ty::Struct(_)
        | Ty::Closure { .. }
        // A fixed-size array is a plain inline struct, so a box can hold one
        // (A-18): `Box<[u8; 4096]>` is a heap allocation of a fixed-size
        // buffer, which is how one is kept alive past its frame.
        | Ty::Array(..)
        // A handle is a pointer, so a box of one is a pointer to a pointer:
        // `Box<Widget>` is the owned spelling of "an optional handle".
        | Ty::Handle(_)
        // A raw pointer is the same shape: `Box<Ptr<i32>>` is a pointer to a
        // pointer, held on the heap.
        | Ty::Ptr(_) => {
            true
        }
        // A `Box` holds a pointer, so its own element slot may hold another
        // container, but only one whose element type is decided: the marker
        // would be a `void` element the generated routines cannot name.
        Ty::List(_) | Ty::Box(_) => !contains_open_container(ty),
        // A `View` is a borrow of text that outlives the box, with nowhere to
        // put the lifetime - the same reason `str` is refused (A-8).
        Ty::Ref { .. } | Ty::Str | Ty::View | Ty::Unit | Ty::Dyn { .. } => false,
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// Whether `ty` may be a `Ptr<T>`'s element: any type with a C/LLVM layout
/// that is one machine-level value, which is what a raw address can point at.
///
/// The owned heap types (`String`, `Bytes`, `List`, `Box`, `Map`), the borrowed
/// ones (`str`, `View`, a reference), a closure, a function value and the
/// builtin fixed-address `Atomic<T>` are all refused: none of them is one
/// machine value, and a pointer to one would be a pointer into a structure whose
/// shape the language has not promised.
pub fn is_ptr_element(ty: &Ty) -> bool {
    match ty {
        Ty::Struct(_) | Ty::Handle(_) | Ty::Ptr(_) => true,
        // An array is inline storage with a fixed size, so a pointer to one is
        // an ordinary pointer to a block.
        Ty::Array(elem, _) => is_ptr_element(elem),
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// What a `Bytes.read_*`/`Bytes.write_*` method name says about the field it
/// moves: the field's byte width, whether the bytes in the buffer are
/// big-endian, and the Kelp type the value has on either side (kernel stage 3,
/// MK9-S8). `None` for any other name, which is how both code generators tell
/// a byte-order access from the older `Bytes` methods (`get`, `set`, ...) that
/// go through the runtime object.
///
/// The name itself carries the whole answer, so the frontend, the removed C backend
/// and `codegen_llvm.rs` read one table rather than each parsing the name and
/// disagreeing about a width.
pub fn bytes_endian_access(op: &str) -> Option<(usize, bool, Ty)> {
    let rest = op.strip_prefix("read_").or_else(|| op.strip_prefix("write_"))?;
    let (value, order) = rest.rsplit_once('_')?;
    let big_endian = match order {
        "be" => true,
        "le" => false,
        _ => return None,
    };
    let width = match value {
        "u16" => 2,
        "u32" | "i32" => 4,
        "u64" | "i64" => 8,
        _ => return None,
    };
    Some((width, big_endian, Ty::from_name(value)?))
}

/// Why a type cannot be a `Ptr<T>`'s element, in one sentence that names the
/// reason the type itself has: an owned value the language manages, a borrow,
/// or a value that is not one machine-level thing.
pub fn ptr_element_error(ty: &Ty) -> String {
    match ty {
        Ty::Str => "'str' is a borrowed view, not an address a pointer can hold".to_string(),
        Ty::View => "'View' is a borrowed view, not an address a pointer can hold".to_string(),
        Ty::Ref { mutable, inner } => format!(
            "'{}' is a borrow: take the address of the *value* with 'Ptr.from_ref'/'Ptr.from_mut' instead",
            Ty::Ref { mutable: *mutable, inner: inner.clone() }.display()
        ),
        Ty::String | Ty::Bytes | Ty::List(_) | Ty::Box(_) | Ty::Map(..) => format!(
            "'{}' is an owned heap value, which is not one machine-level value a raw pointer can address",
            ty.display()
        ),
        Ty::Closure { .. } | Ty::Fn { .. } => format!(
            "'{}' is a function value, not data a pointer can address",
            ty.display()
        ),
        Ty::Atomic(_) => format!(
            "'{}' is an atomic cell, whose operations the language controls; point at the plain value it holds instead",
            ty.display()
        ),
        Ty::Unit => "'unit' is not a value".to_string(),
        other => format!(
            "'{}' is not a type a raw pointer can address: a 'Ptr<T>' element is a scalar, 'bool', 'char', a 'datac'/'enum', a fixed-size array, an 'extern type' handle or another 'Ptr'",
            other.display()
        ),
    }
}

/// The element type of a `Ptr<T>`, looking through a borrow - `p.read()` arrives
/// with the receiver typed `&Ptr<T>` (the receiver may be a borrow of a place).
pub fn ptr_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Ptr(elem) => Some(elem),
        Ty::Ref { inner, .. } => ptr_elem_of(inner),
        _ => None,
    }
}

/// Whether `ty` may be a `Thread<T>`'s element: anything a worker may hand back
/// to its joiner, which is anything that can be moved through the boundary.
///
/// A borrow, a `str` and a `View` are refused for the reason they are refused
/// in every stored position: each points at data somebody else owns, and the
/// frame that owns it ends before the joiner can read the result
/// (`is_borrowed_field_ty` is the same predicate the stored positions use, so
/// this cannot drift from them).
pub fn is_thread_element(ty: &Ty) -> bool {
    !is_borrowed_field_ty(ty)
}

/// Whether `ty` may be an `Atomic<T>`'s element: the integer types and `bool`,
/// and nothing else.
///
/// A float has no atomic hardware operation worth claiming (`Atomic<f32>` would
/// be a compare-and-swap loop, not an atomic add); `String`, `Bytes`, a `datac`
/// and a container each own memory or carry a tag, so a single-word atomic
/// cannot hold one; a borrow and a `View` are somebody else's data. The set is
/// closed rather than open-ended because the C and LLVM lowerings both have to
/// name the element's width, and a closed set is what makes that a total
/// function instead of a diagnostic at codegen time.
pub fn is_atomic_element(ty: &Ty) -> bool {
    matches!(ty, Ty::Bool) || ty.is_integer()
}

/// Whether `ty` may be a `Mutex<T>`'s element: an owned value with no borrows,
/// the same `Send`-shaped predicate `Thread<T>` uses (`is_thread_element`). A
/// `Mutex<T>` shares `T` across threads, so a borrow, a `str` or a `View` in
/// the slot would be a thread reading data another frame owns.
pub fn is_mutex_element(ty: &Ty) -> bool {
    !is_borrowed_field_ty(ty)
        && !matches!(
            ty,
            Ty::Unit | Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. }
        )
}

/// Whether `ty` may be an `RwLock<T>`'s element: the same `Send`-shaped
/// predicate `Mutex<T>` uses. `RwLock<T>` shares `T` across threads, so a
/// borrow, a `str` or a `View` in the slot would be a thread reading data
/// another frame owns.
pub fn is_rwlock_element(ty: &Ty) -> bool {
    is_mutex_element(ty)
}

/// The element type of an `RwLock<T>`, looking through a borrow: `rw.read { ... }`
/// arrives with the receiver typed `&RwLock<T>`.
pub fn rwlock_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::RwLock(elem) => Some(elem),
        Ty::Ref { inner, .. } => rwlock_elem_of(inner),
        _ => None,
    }
}

/// The element type of a `Mutex<T>`, looking through a borrow: `m.lock { ... }`
/// arrives with the receiver typed `&Mutex<T>` (a method receiver is
/// auto-borrowed, and a `pd` mutex is reached through its address).
pub fn mutex_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Mutex(elem) => Some(elem),
        Ty::Ref { inner, .. } => mutex_elem_of(inner),
        _ => None,
    }
}

/// The element type of an `Atomic<T>`, looking through a borrow: `c.load(...)`
/// arrives with the receiver typed `&Atomic<T>` (a method receiver is
/// auto-borrowed, and a `pd` atomic is reached through its address).
pub fn atomic_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Atomic(elem) => Some(elem),
        Ty::Ref { inner, .. } => atomic_elem_of(inner),
        _ => None,
    }
}

/// The `List<T>` or `Box<T>` behind an expression's type, looking through a
/// borrow - `l.push(x)` and `b.get()` both arrive with the receiver typed
/// `&List<T>`/`&Box<T>` (a container parameter is a borrow, and a method
/// receiver is auto-borrowed).
pub fn list_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::List(elem) => Some(elem),
        Ty::Ref { inner, .. } => list_elem_of(inner),
        _ => None,
    }
}

/// Whether this type is an *open* container: one a bare `List.empty()`,
/// `Box.empty()` or `Map.new()` made, with no type written anywhere to say
/// what it holds.
///
/// Such a container carries `Unit` in the slot its element type goes
/// (`List<Unit>`, `Box<Unit>`, `Map<Unit, Unit>`). The marker cannot be
/// confused with a type a program wrote: `is_list_element`, `is_box_element`
/// and `is_map_key` all refuse `Unit`, so `resolve_type` has no way to build
/// one from source. The first operation that has to know the element is
/// therefore free to fill it in (see `fill_open_container`).
pub fn is_open_container(ty: &Ty) -> bool {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => **elem == Ty::Unit,
        Ty::Map(key, value) => **key == Ty::Unit || **value == Ty::Unit,
        Ty::Ref { inner, .. } => is_open_container(inner),
        _ => false,
    }
}

/// The type an open container becomes once `from` says what it holds: `from`
/// is the container a `push`/`insert`/`set` argument built, or the one a
/// surrounding position expects. `None` when the two are not the same kind of
/// container, or when what `from` holds is not something a container may hold,
/// so an open container whose element the caller cannot supply stays open for
/// the check that explains why.
///
/// A slot that is already decided keeps its type: an open container only ever
/// has one, and a `Map` whose key came from the binding's written type keeps
/// that key.
pub fn fill_open_container(
    open: &Ty,
    from: &Ty,
    keyable: &std::collections::HashSet<String>,
) -> Option<Ty> {
    let (open, from) = match (open, from) {
        (Ty::Ref { inner, .. }, _) => (inner.as_ref(), from),
        (_, Ty::Ref { inner, .. }) => (open, inner.as_ref()),
        _ => (open, from),
    };
    match (open, from) {
        (Ty::List(_), Ty::List(elem)) if is_list_element(elem) => Some(Ty::List(elem.clone())),
        (Ty::Box(_), Ty::Box(elem)) if is_box_element(elem) => Some(Ty::Box(elem.clone())),
        (Ty::Map(open_key, open_value), Ty::Map(from_key, from_value)) => {
            if !is_map_key_with(from_key, keyable) || !is_map_element(from_value) {
                return None;
            }
            if **open_key != Ty::Unit && open_key != from_key {
                return None;
            }
            if **open_value != Ty::Unit && open_value != from_value {
                return None;
            }
            Some(Ty::Map(from_key.clone(), from_value.clone()))
        }
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

/// As `list_elem_of`, for `Map<K, V>` - the *value* type. The key is what the
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

/// The element type of an array, looking through a borrow: `a.len()` and
/// `a.get(i)` arrive with the receiver typed `&[T; N]`, since an owned array
/// parameter is a borrow.
pub fn array_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Array(elem, _) => Some(elem),
        Ty::Ref { inner, .. } => array_elem_of(inner),
        _ => None,
    }
}

/// The length of an array, looking through a borrow.
pub fn array_len_of(ty: &Ty) -> Option<u64> {
    match ty {
        Ty::Array(_, len) => Some(*len),
        Ty::Ref { inner, .. } => array_len_of(inner),
        _ => None,
    }
}

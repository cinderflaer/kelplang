use super::*;

/// The name a builtin type answers to in a method key - the non-`datac`
/// counterpart of `struct_name_of`.
pub(crate) fn builtin_type_name(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::Bytes => Some("Bytes"),
        Ty::String => Some("String"),
        // A `str` **value** can now have methods, which it could not before
        // A-8: `s.view()` needs a receiver, and a `str` has no other name to
        // be one (the `std/str` helpers are free functions called
        // `str.len(s)`). Only `str#view` exists, so nothing else changes.
        Ty::Str => Some("str"),
        Ty::View => Some("View"),
        Ty::List(_) => Some("List"),
        Ty::Box(_) => Some("Box"),
        Ty::Array(..) => Some("Array"),
        Ty::Map(..) => Some("Map"),
        // An `Atomic<T>` answers to one name whatever its element, so
        // `c.load(...)` keys on `Atomic#load` and the element type comes from
        // the receiver (`atomic_sig`).
        Ty::Atomic(_) => Some("Atomic"),
        // A `Mutex<T>` answers to one name whatever its element, exactly as an
        // `Atomic<T>` does (B-5, `std/sync`).
        Ty::Mutex(_) => Some("Mutex"),
        // An `RwLock<T>` answers to one name whatever its element, and the
        // non-generic `Condvar` answers to its own.
        Ty::RwLock(_) => Some("RwLock"),
        Ty::Condvar => Some("Condvar"),
        // The integer types answer to their written names so that the
        // conversion methods below (`n.to_u8()`) have a receiver to sit on.
        Ty::I32 => Some("i32"),
        Ty::I64 => Some("i64"),
        Ty::Int => Some("int"),
        Ty::U32 => Some("u32"),
        Ty::U64 => Some("u64"),
        Ty::Uint => Some("uint"),
        Ty::U8 => Some("u8"),
        Ty::U16 => Some("u16"),
        Ty::Usize => Some("usize"),
        Ty::Isize => Some("isize"),
        Ty::F32 => Some("f32"),
        Ty::F64 => Some("f64"),
        Ty::Float => Some("float"),
        // An owned parameter is a borrow (M4's rule: every owned parameter is
        // borrowed), so `b.len()` on one arrives here as `&Bytes`/`&String`.
        Ty::Ref { inner, .. } => builtin_type_name(inner),
        _ => None,
    }
}

/// Why a type cannot be a container element: a borrow (or the `str` view
/// that is the same thing) would outlive the value it points at, and v1 has
/// no lifetime syntax to say otherwise.
pub(crate) fn borrowed_element_reason(ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Str => "'str' is a borrowed view".to_string(),
        // A `View` is the same kind of thing with a length, and it has the
        // same problem: an element outlives the text it points at, and v1 has
        // no lifetime syntax to say otherwise (A-8, A-11).
        Ty::View => "'View' is a borrowed view".to_string(),
        Ty::Ref { mutable, inner } => format!(
            "'{}' is a borrow",
            Ty::Ref { mutable: *mutable, inner: inner.clone() }.display()
        ),
        Ty::Unit => "'unit' is not a value".to_string(),
        _ => return None,
    })
}

/// Why a *function value* cannot be a container element, in one sentence
/// that says the real reason (A-4). It is not the borrow rule and not the
/// inline-storage rule: a function pointer is a plain inline value, and the
/// only thing missing is a C spelling - a container's generated type names its
/// element as a prefix (`kelp_List__i32`), and a function pointer's name goes
/// *inside* the declarator (`int32_t (*kelp_f)(int32_t)`), so there is no such
/// prefix. The way round it is a one-field `datac`, which is what a program
/// needing a table of callbacks should write anyway.
pub(crate) fn fn_element_reason(ty: &Ty, container: &str) -> Option<String> {
    match ty {
        Ty::Fn { .. } => Some(format!(
            "'{}' cannot be a '{container}' element yet - a function value is a plain inline value, but a container's generated C type names its element type as a name prefix, and a function pointer has no such spelling (the name goes inside its declarator); hold it in a one-field 'datac' instead",
            ty.display()
        )),
        // A closure *is* nameable (it is an ordinary C struct), so the honest
        // reason is different: it owns an environment, and a container holds
        // its elements inline, so every element would have to be moved in and
        // dropped out through the container's own generated functions - which
        // is the same shape a function value is refused for, one level up
        // (A-4 phase E).
        Ty::Closure { .. } => Some(format!(
            "'{}' cannot be a '{container}' element yet - a closure owns the environment its captures were moved into, and a container's elements are held inline; put it in a 'Box' or a one-field 'datac' instead",
            ty.display()
        )),
        _ => None,
    }
}

/// Why an `Atomic<T>` cannot be a container element. MK9 9.3 left "may an
/// atomic be a `List` element, a `Map` value, a `datac` field" open, and A-10
/// slice 1 closed it: an atomic stays refused as a `List`/`Map`/`Box`/array
/// element, and a `datac` field of atomic type stays accepted. A container's
/// own routines read and write an element as a plain location, and every access
/// to an atomic has to be one atomic operation; a field is reached by field
/// access, which is where the atomic's own methods sit.
///
/// `container` is the whole phrase the refusal names, with its article, so a
/// `Map` value reads "a 'Map' value" and an array element reads "an array
/// element".
pub(crate) fn atomic_element_reason(ty: &Ty, container: &str) -> Option<String> {
    let Ty::Atomic(elem) = ty else {
        return None;
    };
    Some(format!(
        "'Atomic<{}>' cannot be {container}: the container's own routines read and write it as a plain location, and every access to an atomic has to be one atomic operation. Keep the atomic in a place of its own ('val c = AtomicI64.new(0)') and put its value in the container",
        elem.display()
    ))
}

/// Why a `Thread<T>` cannot be a container element (MK9-S3-M1 stage B1, where
/// `Thread<T>` arrives). The same shape as the atomic refusal above and for a
/// neighbouring reason: a handle names one running worker, and a container's
/// own routines copy and drop an element as a plain location, which would name
/// the worker twice and join it once. A handle is accepted as a local, a field
/// and a parameter, which is where a join has one place to happen.
pub(crate) fn thread_element_reason(ty: &Ty, container: &str) -> Option<String> {
    let Ty::Thread(elem) = ty else {
        return None;
    };
    Some(format!(
        "'Thread<{}>' cannot be {container}: a handle names one running worker and is joined once, while a container's own routines copy and drop an element like a plain location. Keep the handle in a local or a field ('val h = thread.spawn(...)') and join it there",
        elem.display()
    ))
}

/// Why a `Mutex<T>` cannot be a container element (B-5). Mirrors the thread
/// refusal: a mutex owns a runtime record and is reached by borrowing it, while
/// a container's own routines move and drop an element like a plain location.
/// A local, a field or a `pd` gives `lock` one place to borrow.
pub(crate) fn mutex_element_reason(ty: &Ty, container: &str) -> Option<String> {
    let Ty::Mutex(elem) = ty else {
        return None;
    };
    Some(format!(
        "'Mutex<{}>' cannot be {container}: a mutex owns a runtime record and is reached by borrowing it, while a container's own routines move and drop an element like a plain location. Keep the mutex in a local or a field ('val m = Mutex.new(0)') and lock it there",
        elem.display()
    ))
}

/// Why an `RwLock<T>` or a `Condvar` cannot be a container element. Mirrors the
/// mutex refusal for the lock; a `Condvar` is shared by construction and a
/// container's copy-and-drop would name the one record twice.
pub(crate) fn rwlock_element_reason(ty: &Ty, container: &str) -> Option<String> {
    match ty {
        Ty::RwLock(elem) => Some(format!(
            "'RwLock<{}>' cannot be {container}: an rwlock owns a runtime record and is reached by borrowing it, while a container's own routines move and drop an element like a plain location. Keep the rwlock in a local or a field ('val rw = RwLock.new(0)') and borrow it there",
            elem.display()
        )),
        Ty::Condvar => Some(format!(
            "'Condvar' cannot be {container}: a condition variable is a shared runtime record paired with one mutex, while a container's own routines move and drop an element like a plain location. Keep it in a local or a field beside the mutex it pairs with"
        )),
        _ => None,
    }
}

/// Why a container that still carries the open-container marker cannot be a
/// container element, in one sentence that says the real reason: `ty` is such
/// a container, or holds one, so the generated typedef, its `push` and its drop
/// have no element type to name (`is_open_container` and
/// `contains_open_container` in `types.rs`). The check is at any depth, because
/// the outer container's own slot would carry the marker in.
pub(crate) fn open_container_element_reason(ty: &Ty, container: &str) -> Option<String> {
    if !crate::types::contains_open_container(ty) {
        return None;
    }
    Some(format!(
        "'{}' cannot be a '{container}' element: a container whose element type is not decided yet has no element type to name, so push a value into it (or write its element type) before using it as an element",
        ty.display()
    ))
}

/// Why a type cannot be a `List<T>` element, in one place so the `List<T>`
/// type position and `List.of(x)` give the same explanation. A borrow, a view,
/// an atomic, a container nobody decided and a function value each have their
/// own reason; what is left for the fall-through is a `Map` (a map value is not
/// a container either), a fixed-size array (a `List` element is not one) and
/// `unit` (the open-container marker).
pub(crate) fn list_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "List") {
        return reason;
    }
    if let Some(reason) = atomic_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "a 'List' element") {
        return reason;
    }
    if let Some(reason) = open_container_element_reason(ty, "List") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}: a 'List' element is owned by the list, so it has to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', another 'List', or a 'Box'"
        );
    }
    format!(
        "'{}' cannot be a 'List' element: a 'Map', an array, a borrow or a view, an atomic, a function value, and 'unit' are refused. A list holds its elements in its own buffer, so a 'List' or a 'Box' element is fine; use a scalar, 'String', 'Bytes', a 'datac', an 'enum', another 'List', or a 'Box'",
        ty.display()
    )
}

/// Why an `enum` cannot be a `Map` value (the checker refuses it here, so a
/// hosted build does not reach codegen). The checker takes an `enum` as a plain
/// element, but the map's entry record is described through its value's layout,
/// and a hosted build cannot describe an enum there (`gc_descr::pointer_offsets`).
/// A `datac` that holds the enum is refused the same way, so the sentence points
/// at a `List`, which holds an enum as its own element.
pub(crate) fn map_enum_value_error(ty: &Ty) -> String {
    format!(
        "'{0}' cannot be a 'Map' value: a hosted build cannot describe an 'enum' inside a map's entry yet, and a 'datac' that holds the enum is refused the same way. Keep the values in a 'List<{0}>' and store each value's index in the map instead",
        ty.display()
    )
}

/// Why a type cannot be a `Map<V>` value, in one place so the `Map<V>` type
/// position and `Map.new()` share the explanation. The value set is narrower
/// than the `List` element set: a container is refused here, so a `List` or a
/// `Box` value stays out even though it is a legal list element (A-10 slice 1).
pub(crate) fn map_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "Map") {
        return reason;
    }
    if let Some(reason) = atomic_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "a 'Map' value") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}: a 'Map' value is owned by the map, so it has to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', or a 'datac'"
        );
    }
    format!(
        "'{}' cannot be a 'Map' value: the value set is a scalar, 'String', 'Bytes', or a 'datac', which is narrower than a list's (that also takes a 'List', a 'Box' or an 'enum'). A container, an array, a borrow or a view, an atomic, a function value, and 'unit' are refused",
        ty.display()
    )
}

/// Whether a value of `ty` owns memory, for the repeat literal's "the element
/// must own nothing" rule (A-18).
///
/// `needs_drop` answers from the declared owned set, which is computed once
/// before any body is checked, so it cannot know about a *derived* record,
/// whose declaration is created on demand while bodies are checked: a tuple's.
/// A tuple is resolved through `struct_def`, which finds both kinds, and its
/// positions are walked (a nested tuple recurses). Everything else is
/// `needs_drop`'s own answer, which already looks through an array.
pub(crate) fn elem_owns_memory(ctx: &Ctx, ty: &Ty) -> bool {
    if crate::types::needs_drop(ty, &ctx.owned_structs) {
        return true;
    }
    match ty {
        Ty::Struct(name) if crate::types::is_tuple_name(name) => ctx.struct_def(name).map_or(
            false,
            |def| def.fields.iter().any(|f| elem_owns_memory(ctx, &f.ty)),
        ),
        _ => false,
    }
}

/// Why a type cannot be an array's element (A-18). The set is a scalar,
/// `String`, `Bytes`, a `datac`, or another array, since an array holds its
/// elements inline, so `[[u8; 4]; 8]` is a page table and is as expressible as
/// it is in C. A `List`/`Box`/`Map` element is refused: an array is fixed-size,
/// so a container element would make it a variable-size thing it is not (A-10
/// slice 1 left this rule alone even as it widened the `List` element set).
pub(crate) fn array_element_error(ty: &Ty) -> String {
    if let Some(reason) = atomic_element_reason(ty, "an array element") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "an array element") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "an array element") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "an array element") {
        return reason;
    }
    match ty {
        Ty::Fn { .. } => {
            return format!(
                "'{}' cannot be an array element yet: a function value is a plain inline value, but the generated C names an array's element type as a name prefix, and a function pointer has no such spelling (the name goes inside its declarator); hold it in a one-field 'datac' instead",
                ty.display()
            )
        }
        Ty::Closure { .. } => {
            return format!(
                "'{}' cannot be an array element yet: a closure owns the environment its captures were moved into, and an array's elements are held inline; put it in a 'Box' or a one-field 'datac' instead",
                ty.display()
            )
        }
        _ => {}
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}. An array holds its elements inline, so they have to outlive it (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', or another array"
        );
    }
    format!(
        "'{}' cannot be an array element: an array holds its elements inline and is fixed-size, so a 'List'/'Box'/'Map' element is refused here (a container finds its home in a 'List' or a 'Box' instead); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', or a nested array",
        ty.display()
    )
}

/// The same for a `Box<T>` element, which may be a container: a box holds a
/// pointer, so `Box<List<i32>>` and `Box<Box<i32>>` are both fine, provided
/// the inner container's own element type is decided.
pub(crate) fn box_element_error(ty: &Ty) -> String {
    if let Some(reason) = fn_element_reason(ty, "Box") {
        return reason;
    }
    if let Some(reason) = atomic_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = thread_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = mutex_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = rwlock_element_reason(ty, "a 'Box' element") {
        return reason;
    }
    if let Some(reason) = open_container_element_reason(ty, "Box") {
        return reason;
    }
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}: a 'Box' owns its value, so the value has to outlive the box (v1 has no lifetime syntax to say otherwise); use a scalar, 'String', 'Bytes', a 'datac', an 'enum', or another 'Box'/'List'"
        );
    }
    format!(
        "'{}' cannot be a 'Box' element: use a scalar, 'String', 'Bytes', a 'datac', an 'enum', a 'List', or another 'Box'",
        ty.display()
    )
}

/// Why a `Mutex<T>`'s element is refused: the value set is any owned type with
/// no borrows (the `Send`-shaped rule B-5 shares with `Thread<T>`). A borrow,
/// a `str` or a `View` in the slot would be a thread reading data another frame
/// owns.
pub(crate) fn mutex_element_error(ty: &Ty) -> String {
    if crate::types::is_borrowed_field_ty(ty) {
        return format!(
            "'{}' cannot be a 'Mutex' element: a mutex is shared across threads, and a borrow, a 'str' or a 'View' in the slot would be a thread reading data another frame owns. Guard an owned value instead",
            ty.display()
        );
    }
    format!(
        "'{}' cannot be a 'Mutex' element: a mutex guards an owned value, and 'unit' and a function value are refused",
        ty.display()
    )
}

/// Why an `RwLock<T>`'s element is refused: the same `Send`-shaped rule
/// `Mutex<T>` uses. A borrow, a `str` or a `View` in the slot would be a thread
/// reading data another frame owns.
pub(crate) fn rwlock_element_error(ty: &Ty) -> String {
    if crate::types::is_borrowed_field_ty(ty) {
        return format!(
            "'{}' cannot be an 'RwLock' element: an rwlock is shared across threads, and a borrow, a 'str' or a 'View' in the slot would be a thread reading data another frame owns. Guard an owned value instead",
            ty.display()
        );
    }
    format!(
        "'{}' cannot be an 'RwLock' element: an rwlock guards an owned value, and 'unit' and a function value are refused",
        ty.display()
    )
}

/// Why a type cannot be an `Atomic<T>`'s element. The set is closed (`bool` and
/// the integer types) because a single-word atomic cannot hold an owned value,
/// a float has no atomic arithmetic worth claiming, and a borrow is somebody
/// else's data.
pub(crate) fn atomic_element_error(ty: &Ty) -> String {
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}. An 'Atomic<T>' is a location, and a borrow in one would be a location holding somebody else's location; use 'bool' or an integer type"
        );
    }
    format!(
        "'{}' cannot be an 'Atomic' element in v1: the element set is 'bool' and the integer types ('i32', 'i64', 'u32', 'u64', 'u8', 'u16', 'usize', 'isize'), because one atomic operation is one machine-word operation and there is no atomic arithmetic for a float",
        ty.display()
    )
}

/// Why a type cannot be a `Thread<T>`'s element. The element set is everything
/// a worker may hand its joiner, so the only refusals are the borrowed shapes:
/// a value that points at data owned by a frame that ends before the joiner
/// reads the result. The stored positions refuse the same three types
/// (`is_borrowed_field_ty`), which is what keeps this message and theirs in
/// step.
pub(crate) fn thread_element_error(ty: &Ty) -> String {
    if let Some(reason) = borrowed_element_reason(ty) {
        return format!(
            "{reason}. A 'Thread<T>' carries the value its worker returns to the joiner, and that value leaves the worker's frame, so T has to be an owned type; hand an owned value back instead"
        );
    }
    format!(
        "'{}' cannot be a 'Thread' element: a thread handle carries the value its worker returns, so T has to be a type that can be moved out of the worker's frame",
        ty.display()
    )
}

/// The `List<T>` builtins, instantiated for one element type. The element
/// decides two signatures:
///
/// - `get` returns a *copy* for a `Copy` element (`List<i32>` → `i32`) and a
///   shared borrow for an owned one (`List<String>` → `&String`, rooted at
///   the receiver by the same elision rule `.kp` functions use), because
///   copying a `String` out of a list would be an unowned second owner;
/// - `push`/`set`/`pop` take and give the element by value, which is where a
///   move in or out of the list happens (M3's move tracking runs on the
///   ordinary call-argument path, so no new borrow rules were needed).
///
/// A `Map`'s value type is the container's element here, so `elem_clonable` is
/// what `Map#get_or`/`Map#values` are gated on as well.
pub(crate) fn container_sig(
    key: &str,
    elem: &Ty,
    map_key: &Ty,
    array_len: Option<u64>,
    elem_sortable: bool,
    elem_clonable: bool,
) -> Option<FnSig> {
    if key.starts_with("List#") {
        list_sig(key, elem, elem_sortable, elem_clonable)
    } else if key.starts_with("Map#") {
        map_sig(key, map_key, elem, elem_clonable)
    } else if key.starts_with("Array#") {
        array_sig(key, elem, array_len.unwrap_or(0))
    } else {
        box_sig(key, elem)
    }
}

/// The `[T; N]` builtins (A-18), instantiated for one element type. The
/// surface is deliberately small, and `len` is not a function of the length
/// here at all: the length is part of the type, so the signature only has to
/// say that `.len()` is a `usize`; the removed C backend emitted the constant.
///
/// `get` follows `List.get` exactly: a `Copy` element is handed back by value,
/// an owned one by shared borrow (`infer.rs` roots that borrow at the receiver
/// through `builtin_borrow_root`). `set` takes the element by value, and both
/// check the index against the type's own length at runtime.
pub(crate) fn array_sig(key: &str, elem: &Ty, len: u64) -> Option<FnSig> {
    // The receiver's own array type, length included: it is the type the call
    // site's argument has, so the signature has to spell the same one.
    let array = Ty::Array(Box::new(elem.clone()), len);
    let shared = Ty::Ref { mutable: false, inner: Box::new(array.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(array) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let elem_ref = Ty::Ref { mutable: false, inner: Box::new(elem.clone()) };
    Some(match key {
        "Array#len" => FnSig { params: vec![p("self", shared)], ret: Ty::Int },
        "Array#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "Array#get" => FnSig {
            params: vec![p("self", shared), p("i", Ty::Int)],
            ret: if elem.is_copy() { elem.clone() } else { elem_ref },
        },
        "Array#set" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Int), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        _ => return None,
    })
}

/// The `Box<T>` builtins (M8), instantiated for one element type. A box
/// holds one value or nothing at all, which is what makes a recursive type
/// expressible; `take` is how a value moves back out, and `get` borrows the
/// value in place.
pub(crate) fn box_sig(key: &str, elem: &Ty) -> Option<FnSig> {
    let boxed = Ty::Box(Box::new(elem.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(boxed.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(boxed.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let elem_ref = Ty::Ref { mutable: false, inner: Box::new(elem.clone()) };
    Some(match key {
        "Box#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "Box#get" => FnSig {
            params: vec![p("self", shared)],
            ret: if elem.is_copy() { elem.clone() } else { elem_ref },
        },
        "Box#set" => FnSig {
            params: vec![p("self", exclusive), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "Box#take" => FnSig {
            params: vec![p("self", exclusive)],
            ret: elem.clone(),
        },
        _ => return None,
    })
}

/// The written type of an atomic operation's ordering argument: the `Ordering`
/// enum the shipped `std/atomic` module declares, named by that module's key
/// prefix. A builtin signature has no declaration site whose module prefix the
/// resolver could have applied, so the key is built here the same way
/// `lower_try_to` builds `Option`'s, and a program that never imports the
/// module cannot write a value of this type, so the key always names a
/// declaration by the time one is passed.
pub fn atomic_order_ty() -> Ty {
    Ty::Struct(format!(
        "{}Ordering",
        crate::stdlib::key_prefix(crate::stdlib::ATOMIC_MODULE)
    ))
}

/// The type of the failure half of `thread.join`'s result: the `ThreadErr`
/// `datac` the shipped `std/thread` module declares, named by that module's key
/// prefix. Built here for the same reason `atomic_order_ty` builds `Ordering`:
/// a builtin signature has no declaration site for the resolver to qualify a
/// type at, and a program that writes `thread.join` has imported the module, so
/// the key always names a declaration by the time one is reached.
pub fn thread_err_ty() -> Ty {
    Ty::Struct(format!(
        "{}{}",
        crate::stdlib::key_prefix(crate::stdlib::THREAD_MODULE),
        crate::stdlib::THREAD_ERR_NAME
    ))
}

/// The `Atomic<T>` methods (MK9-S3-M1 stage A1), instantiated for one element
/// type. Each takes the ordering as its last argument even though v1 has one
/// legal value: MK9 3 reserves the argument position, so a second variant is a
/// widening of the enum and a change to no signature.
///
/// Two things keep this out of the static table. The element type decides every
/// signature, exactly as it does for a container; and `compare_exchange` hands
/// back `Result<T, T>` (MK9 4: `Ok` the value that was there, `Err` the value
/// that turned out to be there), which is an instance of the stdlib's enum and
/// so has to be instantiated at the use site. The `Result` is named through its
/// module id rather than by the enum's own name, and a program that never
/// imports `std/result` is told to rather than given an internal error, the
/// same rule `try_to_<T>`'s `Option` follows.
///
/// `AtomicBool` gets no `fetch_*` at all: there is no atomic add for a flag, and
/// a `fetch_or` on one would be a way to write `true` without saying so.
///
/// **Every receiver is shared, `store`'s included.** MK9 1 states the reason and
/// 4's `mut` spelling was wrong: an atomic mutates through a shared borrow,
/// which is what leaves it with **no `&mut` path by construction**, which is
/// what makes a `&Atomic<T>` the thing that may cross a thread boundary and what
/// stage B1's `Sync`-shaped rule is built on. A `mut` receiver would give the
/// type a `&mut` path and make that rule unsound.
///
/// This is also what makes a `pd val` atomic the shape a shared counter wants:
/// a passdown declared `val` hands a function a *shared* borrow, which is the
/// borrow stage B1's rule lets cross to a thread. A `pd var` counter works too,
/// including a store followed by a load in one function, because the atomic
/// operations are shared borrows reborrowed from the passdown's hidden
/// reference; what `var` adds is the exclusive borrow a function takes when it
/// assigns to the passdown itself, and that is the borrow a thread argument may
/// not take.
pub(crate) fn atomic_sig(key: &str, elem: &Ty, ctx: &Ctx, span: Span) -> Result<Option<FnSig>> {
    let cell = Ty::Atomic(Box::new(elem.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(cell.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let order = atomic_order_ty();
    Ok(Some(match key {
        "Atomic#new" => FnSig { params: vec![p("value", elem.clone())], ret: cell },
        "Atomic#load" => FnSig {
            params: vec![p("self", shared), p("order", order)],
            ret: elem.clone(),
        },
        "Atomic#store" => FnSig {
            params: vec![p("self", shared), p("value", elem.clone()), p("order", order)],
            ret: Ty::Unit,
        },
        // `swap` and the five `fetch_*`: one shape, because each is a
        // read-modify-write that hands back the value that was there. The
        // ordering comes last, which is the argument position MK9 3 reserves.
        // The integer guard is what keeps `fetch_add` off `AtomicBool`.
        "Atomic#swap"
        | "Atomic#fetch_add"
        | "Atomic#fetch_sub"
        | "Atomic#fetch_and"
        | "Atomic#fetch_or"
        | "Atomic#fetch_xor"
            if key == "Atomic#swap" || elem.is_integer() =>
        {
            FnSig {
                params: vec![
                    p("self", shared),
                    p("value", elem.clone()),
                    p("order", order),
                ],
                ret: elem.clone(),
            }
        }
        "Atomic#compare_exchange" => {
            let result_key = format!(
                "{}Result",
                crate::stdlib::key_prefix(crate::stdlib::RESULT_MODULE)
            );
            if !ctx.env.generics.contains_key(&result_key) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Atomic.compare_exchange' hands back 'Result<T, T>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                        result_key,
                        crate::stdlib::RESULT_MODULE
                    ),
                    span,
                });
            }
            let result = ctx.instantiate_type(&result_key, &[elem.clone(), elem.clone()], span)?;
            FnSig {
                params: vec![
                    p("self", shared),
                    p("current", elem.clone()),
                    p("new", elem.clone()),
                    p("order", order),
                ],
                ret: result,
            }
        }
        _ => return Ok(None),
    }))
}

/// The `Ptr<T>` operations that need an unsafe context (MK9-S2 stage 1, with
/// the fallible `try_alloc` from kernel stage 13).
///
/// `Ptr.alloc`, `Ptr.try_alloc`, `read`, `write`, `read_volatile`,
/// `write_volatile` and `free` touch memory the language did not check: they
/// are the whole reason `unsafe` exists, so each names itself in the refusal.
pub(crate) fn ptr_op_is_unsafe(key: &str) -> bool {
    matches!(
        key,
        "Ptr#read"
            | "Ptr#write"
            | "Ptr#read_volatile"
            | "Ptr#write_volatile"
            | "Ptr#read_unaligned"
            | "Ptr#write_unaligned"
            | "Ptr#alloc"
            | "Ptr#try_alloc"
            | "Ptr#free"
            | "Ptr#free_count"
    )
}

/// The refusal for an unsafe-only pointer operation written where no `unsafe`
/// encloses it. The operation is named, because "unsafe operation" alone would
/// leave the reader to find which call in a statement it was.
pub(crate) fn require_ptr_unsafe(key: &str, span: Span, ctx: &Ctx) -> Result<()> {
    if ctx.unsafe_depth > 0 || !ptr_op_is_unsafe(key) {
        return Ok(());
    }
    let name = key.strip_prefix("Ptr#").unwrap_or(key);
    Err(TypeError {
        help: None,
        message: format!(
            "'Ptr.{name}' is an unsafe operation: it can only be used inside 'unsafe {{ ... }}' or in a function declared 'unsafe fun'"
        ),
        span,
    })
}

/// Refuses a name that reaches a `static mut` outside an `unsafe` context
/// (kernel stage 7). The gate fires where the name is resolved, so every way
/// of touching the object - a read, a write, a field access, a method
/// receiver or an address - goes through it, exactly as `Ptr`'s unsafe
/// operations do.
pub(crate) fn require_static_unsafe(s: &StaticInfo, span: Span, ctx: &Ctx) -> Result<()> {
    if !s.mutable || ctx.unsafe_depth > 0 {
        return Ok(());
    }
    Err(TypeError {
        help: None,
        message: format!(
            "'static mut {}' is an unsafe operation: reading, writing or taking its address can only be done inside 'unsafe {{ ... }}' or in a function declared 'unsafe fun'",
            s.name
        ),
        span,
    })
}

/// Whether a method call receiver is a `Ptr`: the bare type name (`Ptr.null()`,
/// which is an *associated* call) or a value whose type is `Ptr<T>` (which is
/// what lets `p.cast<u8>()` carry a type argument at all).
pub(crate) fn ptr_receiver(receiver: &ast::Expr, ctx: &mut Ctx) -> bool {
    if let ast::Expr::Ident(name, _) = receiver {
        if name == "Ptr" && ctx.lookup(name).is_none() && !is_module_alias(name, ctx) {
            return true;
        }
    }
    check_expr(receiver, ctx, None)
        .map(|e| crate::types::ptr_elem_of(&e.ty).is_some())
        .unwrap_or(false)
}

/// `Ptr.null()`, `Ptr.from_addr(a)`, `Ptr.from_ref(&x)`, `Ptr.from_mut(x)`,
/// `Ptr.alloc<T>(count)` (MK9-S2 stage 1) and `Ptr.try_alloc<T>(count)` (kernel
/// stage 13).
///
/// The element type comes from the call's own `<T>` when it writes one, and
/// otherwise from the binding's written type or the expected type: a constructor
/// that produces an `i32` location has nothing else to read it from, so
/// `val p = Ptr.null()` is refused and the message says where to write it.
pub(crate) fn check_ptr_associated(
    method: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // `Ptr.from_fn(f)` (kernel gap 2): the address of a declared `fun` or a
    // `fun` value with no environment, as a `Ptr<u8>`. It is the one pointer
    // constructor that is *safe*: it names code the compiler already checked,
    // and the result is an address, not a call. Its element is always `u8`, so
    // it never reads the call's own type argument or the expected type.
    if method == "from_fn" {
        if !type_args.is_empty() {
            return Err(TypeError {
                help: None,
                message: "'Ptr.from_fn' takes no type argument: its result is always 'Ptr<u8>'"
                    .to_string(),
                span,
            });
        }
        let [arg] = args else {
            return Err(TypeError {
                help: None,
                message: format!("'Ptr.from_fn' takes exactly 1 argument, found {}", args.len()),
                span,
            });
        };
        let value = check_expr(&arg.value, ctx, None)?;
        match &value.ty {
            Ty::Fn { .. } => {}
            Ty::Closure { .. } => {
                return Err(TypeError {
                    help: None,
                    message: "'Ptr.from_fn' takes a function, but this is a closure: its body captured something, so it carries an environment the code address alone cannot describe; pass a declared 'fun' or a 'fun' literal that captures nothing".to_string(),
                    span,
                })
            }
            Ty::ScopedFn { .. } => {
                return Err(TypeError {
                    help: None,
                    message: "'Ptr.from_fn' takes a function, but this is a scoped function value: it is a code pointer paired with a stack environment that lives only for the call it was written in, so it has no standalone code address".to_string(),
                    span,
                })
            }
            other => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Ptr.from_fn' takes a function value, found '{}': name a declared 'fun' or write a 'fun(...)' literal that captures nothing",
                        other.display()
                    ),
                    span,
                })
            }
        }
        return Ok(tir::Expr {
            kind: tir::ExprKind::Call {
                callee: "Ptr#from_fn".to_string(),
                args: vec![value],
            },
            ty: Ty::Ptr(Box::new(Ty::U8)),
            span,
        });
    }
    let explicit = resolve_explicit_args(type_args, ctx)?;
    let elem = match explicit.len() {
        0 => expected.and_then(crate::types::ptr_elem_of).cloned(),
        1 => Some(explicit[0].clone()),
        n => {
            return Err(TypeError {
                help: None,
                message: format!("'Ptr' takes exactly 1 type argument, found {n}"),
                span,
            })
        }
    };
    let Some(elem) = elem else {
        return Err(TypeError {
            help: None,
            message: format!(
                "cannot tell which type 'Ptr.{method}' points at: write it in the call ('Ptr.{method}<i32>(...)' where the operation takes a type argument) or on the binding ('val p: Ptr<i32> = Ptr.null()')"
            ),
            span,
        });
    };
    if !crate::types::is_ptr_element(&elem) {
        return Err(TypeError {
            help: None,
            message: crate::types::ptr_element_error(&elem),
            span,
        });
    }
    let ptr = Ty::Ptr(Box::new(elem.clone()));
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let (key, params, ret) = match method {
        "null" => ("Ptr#null".to_string(), Vec::new(), ptr),
        "from_addr" => (
            "Ptr#from_addr".to_string(),
            vec![p("addr", Ty::Usize)],
            ptr,
        ),
        "from_ref" => (
            "Ptr#from_ref".to_string(),
            vec![p("place", Ty::Ref { mutable: false, inner: Box::new(elem) })],
            ptr,
        ),
        "from_mut" => (
            "Ptr#from_mut".to_string(),
            vec![p("place", Ty::Ref { mutable: true, inner: Box::new(elem) })],
            ptr,
        ),
        "alloc" => ("Ptr#alloc".to_string(), vec![p("count", Ty::Usize)], ptr.clone()),
        // `Ptr.try_alloc<T>(count)` (kernel stage 13): the fallible twin of
        // `Ptr.alloc`, so the same element resolution and the same signature,
        // only a different callee key.
        "try_alloc" => ("Ptr#try_alloc".to_string(), vec![p("count", Ty::Usize)], ptr),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'Ptr.{other}' is not a pointer operation: a pointer is made with 'Ptr.null()', 'Ptr.from_addr(a)', 'Ptr.from_ref(&x)', 'Ptr.from_mut(x)', 'Ptr.alloc<i32>(count)' or 'Ptr.try_alloc<i32>(count)', and read through the value it is bound to"
                ),
                span,
            })
        }
    };
    require_ptr_unsafe(&key, span, ctx)?;
    let checked = match_args(&format!("'Ptr.{method}'"), &params, args, span, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked },
        ty: ret,
        span,
    })
}

use super::*;

/// A method on a `Ptr<T>` value: `addr`, `is_null`, `offset`, `byte_offset`,
/// `cast<U>`, `read`, `write`, `read_volatile`, `write_volatile`,
/// `read_unaligned`, `write_unaligned` and `free` (MK9-S2 stage 1, with the
/// two unaligned forms from kernel stage 3).
pub(crate) fn check_ptr_method(
    probe: tir::Expr,
    receiver: &ast::Expr,
    method: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let elem = crate::types::ptr_elem_of(&probe.ty)
        .cloned()
        .expect("the caller matched a Ptr receiver");
    let explicit = resolve_explicit_args(type_args, ctx)?;
    let cast_to = if method == "cast" {
        match explicit.len() {
            1 => Some(explicit[0].clone()),
            n => {
                return Err(TypeError {
                    help: None,
                    message: format!("'Ptr.cast' takes exactly 1 type argument, found {n}"),
                    span,
                })
            }
        }
    } else {
        if !explicit.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'Ptr.{method}' does not take type arguments - a method's type parameters come from its receiver"
                ),
                span,
            });
        }
        None
    };
    if let Some(target) = &cast_to {
        if !crate::types::is_ptr_element(target) {
            return Err(TypeError {
                help: None,
                message: crate::types::ptr_element_error(target),
                span,
            });
        }
    }
    // `Ptr.free_count` exists only in a freestanding build: a hosted free has
    // no project `dealloc` to hand a size to.
    if method == "free_count" && !ctx.freestanding {
        return Err(TypeError {
            help: Some("in a hosted build, free the pointer with 'Ptr.free()'".to_string()),
            message: "'Ptr.free_count' is only available in a freestanding build: it releases the block through the project's '[target] dealloc', and a hosted build has none".to_string(),
            span,
        });
    }
    let key = format!("Ptr#{method}");
    let Some(sig) = ptr_sig(&key, &elem, cast_to.as_ref(), span)? else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'Ptr.{method}' is not a pointer operation: a pointer has addr, is_null, offset, byte_offset, cast<U>, read, write, read_volatile, write_volatile, read_unaligned, write_unaligned, free and free_count"
            ),
            span,
        });
    };
    require_ptr_unsafe(&key, span, ctx)?;
    let mut checked = vec![check_call_arg("'self'", &sig.params[0].1, receiver, ctx)?];
    checked.extend(match_args(
        &format!("method 'Ptr.{method}'"),
        &sig.params[1..],
        args,
        span,
        ctx,
    )?);
    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked },
        ty: sig.ret,
        span,
    })
}

/// The `Ptr<T>` value methods, instantiated for one element type (MK9-S2 stage
/// 1). Every receiver is written by value: a `Ptr<T>` is `Copy`, so passing one
/// moves nothing and the borrow checker has nothing to add here.
///
/// `read` and `read_volatile` hand back a `T`, which is only sound when `T` is
/// `Copy`: a read of an owned value would produce a second owner of the same
/// memory, and `write` would drop nothing, so the two would leak or double-free
/// depending on which came first. The refusal names that instead of leaving a
/// type error.
pub(crate) fn ptr_sig(key: &str, elem: &Ty, cast_to: Option<&Ty>, span: Span) -> Result<Option<FnSig>> {
    let ptr = Ty::Ptr(Box::new(elem.clone()));
    let p = |n: &str, t: Ty| (n.to_string(), t);
    Ok(Some(match key {
        "Ptr#addr" => FnSig { params: vec![p("self", ptr)], ret: Ty::Usize },
        "Ptr#is_null" => FnSig { params: vec![p("self", ptr)], ret: Ty::Bool },
        "Ptr#offset" | "Ptr#byte_offset" => FnSig {
            params: vec![p("self", ptr.clone()), p("n", Ty::Isize)],
            ret: ptr,
        },
        "Ptr#cast" => {
            let target = cast_to.expect("Ptr.cast resolves its target before its signature");
            FnSig { params: vec![p("self", ptr)], ret: Ty::Ptr(Box::new(target.clone())) }
        }
        "Ptr#free" => FnSig { params: vec![p("self", ptr)], ret: Ty::Unit },
        // `Ptr.free_count(count)` (freestanding only): the sized free. The
        // count is the one `Ptr.alloc`/`try_alloc` was called with, so the
        // project's `dealloc` gets the byte size and alignment the block was
        // allocated with.
        "Ptr#free_count" => FnSig {
            params: vec![p("self", ptr), p("count", Ty::Usize)],
            ret: Ty::Unit,
        },
        "Ptr#read" | "Ptr#read_volatile" | "Ptr#read_unaligned" => {
            if !elem.is_copy() {
                let op = key.strip_prefix("Ptr#").unwrap_or(key);
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'Ptr<{}>.{op}' would read a '{}' out of raw memory, and '{}' is not 'Copy': a second owner of the same value would be made, so the memory could be freed twice. Read a 'Copy' element (a scalar, a handle, a pointer) instead",
                        elem.display(),
                        elem.display(),
                        elem.display()
                    ),
                    span,
                });
            }
            FnSig { params: vec![p("self", ptr)], ret: elem.clone() }
        }
        "Ptr#write" | "Ptr#write_volatile" | "Ptr#write_unaligned" => FnSig {
            params: vec![p("self", ptr), p("value", elem.clone())],
            ret: Ty::Unit,
        },
        _ => return Ok(None),
    }))
}

/// `sortable`/`clonable` are what `types::value_is_sortable` and
/// `value_is_clonable` say about `elem` with `ctx`'s own declaration graph, so
/// a `datac`/`enum` element whose fields are all copyable or all ordered
/// reaches the same two methods a scalar does (MISSING-jL9tE).
pub(crate) fn list_sig(key: &str, elem: &Ty, sortable: bool, clonable: bool) -> Option<FnSig> {
    let list = Ty::List(Box::new(elem.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(list.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(list.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let elem_ref = Ty::Ref { mutable: false, inner: Box::new(elem.clone()) };
    Some(match key {
        "List#len" => FnSig { params: vec![p("self", shared)], ret: Ty::Usize },
        "List#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "List#push" => FnSig {
            params: vec![p("self", exclusive), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "List#get" => FnSig {
            params: vec![p("self", shared), p("i", Ty::Usize)],
            ret: if elem.is_copy() { elem.clone() } else { elem_ref },
        },
        "List#set" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Usize), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "List#pop" => FnSig {
            params: vec![p("self", exclusive)],
            ret: elem.clone(),
        },
        "List#clear" => FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit },
        // S1's additions. Three of them are only meaningful for an element
        // type that has an order / equality / a clone, so an unsupported
        // element type falls through to the "no method" diagnostic rather
        // than getting a signature codegen cannot emit (see
        // `types::list_sortable_elem` and friends).
        "List#sort" if sortable => {
            FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit }
        }
        "List#reverse" => FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit },
        "List#contains" if crate::types::list_comparable_elem(elem) => FnSig {
            params: vec![p("self", shared), p("v", contains_param(elem))],
            ret: Ty::Bool,
        },
        "List#slice" if clonable => FnSig {
            params: vec![p("self", shared), p("from", Ty::Usize), p("to", Ty::Usize)],
            ret: list.clone(),
        },
        // `clone()` under the name a caller reaches for first: the copy
        // `slice(0, len)` already makes, under the same clonable gate, so an
        // element that cannot be copied out gets the "no method" diagnostic
        // rather than a signature codegen has no body for.
        "List#clone" if clonable => {
            FnSig { params: vec![p("self", shared)], ret: list.clone() }
        }
        "List#insert" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Usize), p("v", elem.clone())],
            ret: Ty::Unit,
        },
        "List#remove" => FnSig {
            params: vec![p("self", exclusive), p("i", Ty::Usize)],
            ret: elem.clone(),
        },
        _ => return None,
    })
}

/// The `Map<V>` builtins (MK5-S5), instantiated for one key and value type.
/// A `String` key is passed as a `str` view, a scalar by value and a user key
/// by shared borrow; the value type decides the same things the `List` element
/// type does:
///
/// - `get` returns a *copy* for a `Copy` value and a shared borrow for an
///   owned one, rooted at the receiver by the elision rule (`Map#get` is in
///   `builtin_borrow_root` for exactly this reason);
/// - `get_or` and `values` have to copy or clone a value *out*, so they are
///   offered only for a clonable value type, by the same declaration-graph
///   predicate (`types::value_is_clonable`) that gates `List.slice`, which
///   reaches a `datac`/`enum` whose fields all deep-copy (MISSING-qM4Zx);
/// - `insert` moves the value in, `remove` moves it out.
pub(crate) fn map_sig(key: &str, map_key: &Ty, value: &Ty, clonable: bool) -> Option<FnSig> {
    let map = Ty::Map(Box::new(map_key.clone()), Box::new(value.clone()));
    let shared = Ty::Ref { mutable: false, inner: Box::new(map.clone()) };
    let exclusive = Ty::Ref { mutable: true, inner: Box::new(map.clone()) };
    let p = |n: &str, t: Ty| (n.to_string(), t);
    let value_ref = Ty::Ref { mutable: false, inner: Box::new(value.clone()) };
    // A `String` key is passed as a view and copied in by the map; a scalar
    // key is passed by value.
    let key_param = map_key_param(map_key);
    Some(match key {
        "Map#len" => FnSig { params: vec![p("self", shared)], ret: Ty::Usize },
        "Map#is_empty" => FnSig { params: vec![p("self", shared)], ret: Ty::Bool },
        "Map#has" => FnSig {
            params: vec![p("self", shared), p("key", key_param.clone())],
            ret: Ty::Bool,
        },
        "Map#get" => FnSig {
            params: vec![p("self", shared), p("key", key_param.clone())],
            ret: if value.is_copy() { value.clone() } else { value_ref },
        },
        "Map#get_or" if clonable => FnSig {
            params: vec![p("self", shared), p("key", key_param.clone()), p("fallback", value.clone())],
            ret: value.clone(),
        },
        "Map#insert" => FnSig {
            params: vec![p("self", exclusive), p("key", key_param.clone()), p("v", value.clone())],
            ret: Ty::Unit,
        },
        "Map#remove" => FnSig {
            params: vec![p("self", exclusive), p("key", key_param.clone())],
            ret: value.clone(),
        },
        "Map#clear" => FnSig { params: vec![p("self", exclusive)], ret: Ty::Unit },
        "Map#keys" => FnSig {
            params: vec![p("self", shared)],
            ret: Ty::List(Box::new(map_key.clone())),
        },
        "Map#values" if clonable => FnSig {
            params: vec![p("self", shared)],
            ret: Ty::List(Box::new(value.clone())),
        },
        _ => return None,
    })
}

/// Why a type cannot be a `Map` key (A-2, A-6, A-12). The compiler derives the
/// key's hash and its equality from the fields, and the only way past a field
/// the derivation refuses is the declared pair, so the message names it: this
/// is the one refusal a caller's next edit can answer.
pub(crate) fn map_key_error(ty: &Ty) -> String {
    format!(
        "'{}' cannot be a Map key: the key has to be 'String', an integer, 'bool' or 'char', or a 'datac'/'enum' whose every field is one of those, because the compiler derives the key's hash and its equality from the fields; a 'datac' or 'enum' outside that set becomes a key by declaring 'fun T.hash() => u64' and 'fun T.eq(other: &T) => bool'",
        ty.display()
    )
}

/// How a `Map`'s key is passed to its own methods: a `String` key is a `str`
/// view (the map copies it in, and a view is what a `str` caller already has),
/// a scalar key is passed by value, and a user key is a shared borrow the map
/// clones from (passing it by value would move the caller's key, or copy an
/// owned field it does not own twice).
pub(crate) fn map_key_param(key: &Ty) -> Ty {
    match key {
        Ty::String => Ty::Str,
        Ty::Struct(_) => Ty::Ref { mutable: false, inner: Box::new(key.clone()) },
        other => other.clone(),
    }
}

/// How `List.contains` takes its needle: a `Copy` element by value, an owned
/// one by borrow. Taking a `String` by value would move it into a membership
/// test, which is never what the caller meant.
pub(crate) fn contains_param(elem: &Ty) -> Ty {
    if elem.is_copy() {
        elem.clone()
    } else {
        Ty::Ref { mutable: false, inner: Box::new(elem.clone()) }
    }
}

/// One entry of the `BUILTINS` table as a listing: the key (`String#len`), the
/// type and member names it splits into, whether it is called on the type
/// (`String.from_str`) rather than on a value, and its parameters and result
/// as the type checker spells them. `kelpc doc --format json` writes these.
pub struct BuiltinSig {
    pub key: String,
    pub type_name: String,
    pub name: String,
    pub associated: bool,
    /// `(name, type)`, the receiver first when there is one.
    pub params: Vec<(String, String)>,
    pub ret: String,
}

/// Every `BUILTINS` entry, sorted by key. `List` and `Map` members are not in
/// the table: their signatures depend on the element type, so `list_sig` and
/// `map_sig` build them on demand.
pub fn builtin_signatures() -> Vec<BuiltinSig> {
    let mut out: Vec<BuiltinSig> = BUILTINS
        .iter()
        .map(|(key, b)| {
            let (type_name, name) = key.split_once('#').unwrap_or(("", key.as_str()));
            BuiltinSig {
                key: key.clone(),
                type_name: type_name.to_string(),
                name: name.to_string(),
                associated: b.associated,
                params: b.params.iter().map(|(n, t)| (n.clone(), t.display())).collect(),
                ret: b.ret.display(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// The signature a builtin key resolves to, if it is one.
pub(crate) fn builtin_sig(key: &str) -> Option<FnSig> {
    BUILTINS.get(key).map(|b| FnSig {
        params: b.params.clone(),
        ret: b.ret.clone(),
    })
}

/// For a callee whose result is a *borrow*, the parameter index the elided
/// lifetime binds to - always the first reference parameter, which is why
/// this only has to answer for functions that return one at all.
/// `borrowck.rs` needs it at every return site and every call site; builtins
/// share the rule with `.kp` functions.
pub fn borrow_root_param(params: &[(String, Ty)]) -> Option<usize> {
    params.iter().position(|(_, t)| is_ref_param(t))
}

/// Whether a parameter is a reference the elision rule can borrow from.
/// `str` and `View` count: a `str` parameter *is* `&str`, and a `View` is a
/// `&str` with a length, both modelled as flat borrowed values (`Ty::Str`,
/// `Ty::View`) rather than a `Ty::Ref`. That is the same set as
/// `types::is_borrowed_field_ty` - a type that points at somebody else's data
/// cannot be a parameter's elision root unless it is also a type you cannot
/// store, which is the same fact stated twice.
pub fn is_ref_param(ty: &Ty) -> bool {
    crate::types::is_borrowed_field_ty(ty)
}

/// Whether a value of this type **borrows**: the result of a function that
/// returns one has to be rooted in a parameter (the elision rule), and the
/// caller has to keep that owner borrowed while the value lives.
///
/// `Ty::Ref` is the general case. `Ty::View` is the flat one: it is a pointer
/// and a length passed *by value*, so it is not a `Ty::Ref` in the type
/// system, but it points at text somebody else owns exactly as `&str` does -
/// which is why every rule that applies to a returned `&str` has to apply to a
/// returned `View` (A-8). `borrowck.rs` keys those rules on this predicate
/// rather than on `Ty::Ref` alone.
pub fn is_borrowed_value(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { .. } | Ty::View)
}

/// The elided borrow root for a builtin method, if it returns a borrow.
pub fn builtin_borrow_root(key: &str) -> Option<Option<usize>> {
    // `List#get`/`Box#get`/`Map#get` on an owned element type return a
    // borrow of the container itself - the first parameter, like every
    // builtin method's `self`. (For a `Copy` element the type checker never
    // asks, since the call's result is not a borrow; see
    // `borrowck::check_call`.)
    if key == "List#get" || key == "Box#get" || key == "Map#get" || key == "Array#get" {
        return Some(Some(0));
    }
    let b = BUILTINS.get(key)?;
    // A method that returns a *view* borrows its receiver just as one that
    // returns a `Ty::Ref` does: `s.view()`, `v.slice(a, b)`. `View` is passed
    // by value (two words of `Copy` data), so it is not a `Ty::Ref` in the
    // type system, but the value still points into the receiver.
    if crate::infer::is_borrowed_value(&b.ret) {
        Some(borrow_root_param(&b.params))
    } else {
        None
    }
}

/// A `str` and a `&str` are the same value, so a function declaring one may
/// return the other; whether a *borrowed* return is reachable is the borrow
/// checker's call, not the type's (see `borrowck::check_return_borrow`).
pub(crate) fn str_view_compatible(actual: &Ty, declared: &Ty) -> bool {
    join_value_type(actual, declared).is_some()
}

/// Whether a tail expression diverges, so the function never returns through
/// it and it satisfies any declared return type - including a borrowed one.
/// This is the `panic` ending every payload reader has:
///
/// ```kelp
/// fun as_str(v: &Json) => &str {
///     if let Str(s) = v {
///         return s.as_str()
///     }
///     panic("json.as_str: value is not a string")
/// }
/// ```
///
/// `infer.rs` has had this rule since `Option`/`Result` shipped ("a body whose
/// tail is a `panic` diverges, so it satisfies any declared return type"). The
/// borrow checker's tail check did not, so it read the `panic` as the returned
/// value and refused it for not being derived from a reference parameter. One
/// predicate, both callers: two passes disagreeing about which expression is
/// the returned value is not a bug either pass can see on its own.
pub fn tail_diverges(expr: &tir::Expr) -> bool {
    matches!(&expr.kind, tir::ExprKind::Call { callee, .. } if callee == "panic")
}

/// The type two values of these types have in common, or `None` when they do
/// not agree: `==`, with the one relaxation `str` and `&str` get.
///
/// `is_str_like` is the language's own statement that those two are one type -
/// `str.eq`, `print`, `str.len`, a `str` parameter and a `&str` return all take
/// either, because both are the same C value (`const char*`). Returns already
/// said so (`str_view_compatible`), but arm unification compared them by
/// identity, so ordinary code was refused:
///
/// ```kelp
/// fun f(v: &J) => &str = when v {
///     S(s) -> s.as_str()
///     N -> "n"
/// }
/// // error: 'when' arms have different types: '&str' and 'str'
/// ```
///
/// The join is the **borrow**, not the bare `str`: the borrow checker
/// understands `Ty::Ref` - it pins the owner and it drives the elision rule -
/// and does not understand `Ty::Str`, so a value that might be a view of
/// somebody else's text has to be typed as one. The two are the same C value,
/// so nothing else about the program changes. An *exclusive* `&mut str` is not
/// joined, so this cannot quietly downgrade one to a shared view.
pub(crate) fn join_value_type(left: &Ty, right: &Ty) -> Option<Ty> {
    if left == right {
        return Some(left.clone());
    }
    let exclusive = |ty: &Ty| matches!(ty, Ty::Ref { mutable: true, .. });
    if is_str_like(left) && is_str_like(right) && !exclusive(left) && !exclusive(right) {
        return Some(Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        });
    }
    None
}

/// Whether a type is `str` or a borrow of one - the two spellings of the
/// same borrowed view, which `print` and argument matching treat alike.
pub(crate) fn is_str_like(ty: &Ty) -> bool {
    match ty {
        Ty::Str => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Str),
        _ => false,
    }
}

/// A `char` value and an integer do not mix, and the reason is easy to miss:
/// a character literal takes whatever type its context asks for, so with
/// nothing to ask, it is `char`. Every such mismatch gets the same sentence
/// saying what to write instead.
pub(crate) fn char_integer_hint(found: &Ty, expected: &Ty) -> String {
    if let (Ty::Array(fe, fl), Ty::Array(ee, el)) = (found, expected) {
        if fe == ee && fl != el {
            return " - an array's length is part of its type; pass an array of the expected length, or declare the parameter over any length (`fun f<const N: usize>(a: &[T; N])`)".to_string();
        }
    }
    if matches!(found, Ty::Char) && expected.is_integer() {
        " - a character literal takes the type its context asks for, so with nothing to ask it is 'char'; give it the type you mean (`val c: u8 = '\"'`) or write the value as an integer".to_string()
    } else {
        String::new()
    }
}

/// The direction 5.1 does *not* make implicit: a `str`/`View` becomes an
/// owned `String`/`Bytes` automatically, but the reverse needs an explicit
/// `.as_str()`/`.view()` - `print` and `.as_str()` on a bound `String` both
/// look like the conversion happens for free, so a caller who has only met
/// those two reasonably expects `str.concat(a_string, ...)` to work the same
/// way, and the type-mismatch alone (`'String'`, expected 'str'`) does not
/// say why one direction is free and the other is not.
pub(crate) fn str_view_hint(found: &Ty, expected: &Ty) -> String {
    if !matches!(found, Ty::String) {
        return String::new();
    }
    let method = match expected {
        Ty::Str => "as_str()",
        Ty::View => "view()",
        _ => return String::new(),
    };
    format!(
        " - there is no implicit conversion this direction (5.1 only goes the other way, 'str'/'View' to an owned 'String'); call `.{method}` to borrow it"
    )
}

/// Whether the character literal `cp` can be a value of the integer type
/// `ty`. A character literal takes whatever integer type its context asks
/// for, and this is the one thing that can go wrong with that: `'中'` is a
/// value a `u8` cannot hold.
pub(crate) fn char_fits(cp: u32, ty: &Ty) -> bool {
    match ty {
        Ty::U8 => cp <= u8::MAX as u32,
        Ty::U16 => cp <= u16::MAX as u32,
        Ty::I32 => cp <= i32::MAX as u32,
        // `u32`/`i64`/`u64`/`usize`/`isize` hold every Unicode scalar value,
        // and `f32`/`f64`/`bool`/`char` are not integer targets at all (the
        // caller only asks this for an integer type).
        _ => true,
    }
}

/// The type an integer literal takes, and whether that is the `i32` a literal
/// falls back to when nothing determines one (syntax.md 2.4) rather than a
/// type its context supplied. `check_expr_inner` and the constant evaluator
/// have to answer this the same way, because both refuse an out-of-range
/// literal and the refusal says which of the two applied.
pub(crate) fn int_lit_ty(expected: Option<&Ty>) -> (Ty, bool) {
    match expected {
        Some(t) if t.is_integer() => (t.clone(), false),
        _ => (Ty::I32, true),
    }
}

/// The narrowest integer type that holds `value`, for the repair line of a
/// literal that does not fit the type it landed on. Every value the lexer
/// yields is within `u64`'s range (the widest integer type), and a sign only
/// adds the minimum, so this always names a type that holds the value.
pub(crate) fn int_lit_repair(value: i128) -> &'static str {
    if value < 0 {
        if value >= i32::MIN as i128 {
            "i32"
        } else {
            "i64"
        }
    } else if value <= u8::MAX as i128 {
        "u8"
    } else if value <= u16::MAX as i128 {
        "u16"
    } else if value <= u32::MAX as i128 {
        "u32"
    } else {
        "u64"
    }
}

/// Refuse an integer literal whose value does not fit the type it landed on
/// (BUG-t6Mz2). The type is fixed by this point, and past it the value is cut
/// to that type: a literal that does not fit leaves a different number behind,
/// at a line that is not the one that is wrong, so the refusal stands here.
pub(crate) fn check_int_literal(value: i128, ty: &Ty, defaulted: bool, span: Span) -> Result<()> {
    let Some((min, max)) = Ty::int_bounds(ty) else {
        return Ok(());
    };
    if value >= min && value <= max as i128 {
        return Ok(());
    }
    let repair = int_lit_repair(value);
    let message = if defaulted {
        format!(
            "'{value}' does not fit 'i32', the type a literal takes when nothing determines one: write the type it needs, as in 'val big: {repair} = {value}'"
        )
    } else {
        format!(
            "'{value}' does not fit '{}': write a type that holds it, as in 'val big: {repair} = {value}'",
            ty.display()
        )
    };
    Err(TypeError { message, span, help: None })
}

/// A code point as a diagnostic can show it: the character itself when it is
/// printable, `U+XXXX` otherwise (a control character has nothing to show).
pub(crate) fn printable_codepoint(cp: u32) -> String {
    match char::from_u32(cp) {
        Some(c) if !c.is_control() => c.to_string(),
        _ => format!("U+{cp:04X}"),
    }
}

/// One checked compile-time constant (M6.3). Folded at its declaration and
/// substituted at every use, so it has no storage, no initializer function,
/// and no runtime cost - `math.PI` is the literal `3.141592653589793` in the
/// generated C.
#[derive(Debug, Clone)]
pub(crate) struct ConstInfo {
    pub(crate) value: tir::Expr,
}
/// A folded constant value (A-19). Deliberately small: a compile-time value is
/// a scalar, so there is nothing to allocate and nothing to drop.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConstValue {
    /// Every integer width, `u64`'s whole range included, which is why the
    /// payload is `i128` rather than `i64`: the width and the signedness live
    /// in the `Ty` carried beside the value, and the arithmetic wraps at that
    /// width instead of guessing one.
    Int(i128),
    Float(f64),
    Bool(bool),
    /// A character constant's code point, kept apart from `Int` so that
    /// `const C: char = 'a'` substitutes as a character literal rather than as
    /// the integer `97`.
    Char(u32),
    Str(String),
    /// A tuple of constants, each with its own type: `const ORIGIN: (i32, i32)
    /// = (1, 2)`. It substitutes as the record literal the tuple is, so a use
    /// site pays nothing a runtime tuple would not.
    Tuple(Vec<(ConstValue, Ty)>),
    /// The value of an expression that has none: a `fun` called for its effect
    /// (or one that fell off its end), which is what makes `Unit` a result a
    /// caller has to handle rather than a case that cannot arise.
    Unit,
}

/// The functions a compile-time evaluator may call (A-19): one entry per
/// declared, non-generic `fun`, holding only what a compile-time call needs.
///
/// Built once, before anything is folded, because an initializer may name a
/// function declared *later* in the file, because a `const` and a `fun` are not in
/// each other's declaration order. What the evaluator does with an entry is
/// cross the body against its own step budget; what it will not do is reach
/// code it cannot see, and a call to an `extern` or to a stdlib overload is
/// refused with a message that says so.
#[derive(Default)]
pub(crate) struct ConstFns {
    pub(crate) fns: HashMap<String, ConstFn>,
}

/// One function a const evaluator may call: its parameters, its declared
/// return type, and its body, all as written. Nothing here is resolved yet:
/// a parameter's type is resolved when a call binds an argument to it, which
/// is the first moment it matters.
pub(crate) struct ConstFn {
    pub(crate) params: Vec<(String, ast::TypeExpr)>,
    pub(crate) ret: Option<ast::TypeExpr>,
    pub(crate) body: ast::Block,
    /// Symbol prefix of the module the body was written in, and that module's
    /// alias table. Carried here rather than looked up per call so the
    /// evaluator needs no `Project`: a name inside a called body resolves the
    /// same way it does when that module is the one being compiled.
    pub(crate) prefix: String,
    pub(crate) aliases: HashMap<String, String>,
}

/// Collects the compile-time callable functions (A-19). Generic functions are
/// left out on purpose: an instantiation's parameter types depend on the call
/// site, and a const initializer has no type arguments to offer.
pub(crate) fn collect_const_fns(program: &ast::Program, project: &Project) -> ConstFns {
    let mut out = ConstFns::default();
    for f in &program.functions {
        if !f.type_params.is_empty() || f.receiver.is_some() {
            continue;
        }
        if out.fns.contains_key(&f.name) {
            continue;
        }
        out.fns.insert(
            f.name.clone(),
            ConstFn {
                params: f.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
                ret: match &f.return_type {
                    Some(ast::TypeExpr { name, .. }) if name == "unit" => None,
                    Some(t) => Some(t.clone()),
                    None => None,
                },
                prefix: project.prefix_of(&f.module).to_string(),
                aliases: project.alias_prefixes_of(&f.module),
                body: match &f.body {
                    ast::FunctionBody::Block(b) => b.clone(),
                    // `fun f() = expr` is the same body one statement long.
                    ast::FunctionBody::Expr(e) => ast::Block {
                        stmts: vec![ast::Stmt::Expr {
                            expr: e.clone(),
                            span: e.span(),
                        }],
                        span: e.span(),
                    },
                    // An `extern fun` has no body to run at compile time.
                    ast::FunctionBody::Extern => continue,
                    // A trait signature has no body at all.
                    ast::FunctionBody::Signature => continue,
                },
            },
        );
    }
    out
}

/// Stand-in for "no type parameters in scope" at every site that is not
/// inside a generic template's own fields (A-2).
pub(crate) static EMPTY_PARAMS: std::sync::LazyLock<HashMap<String, Ty>> =
    std::sync::LazyLock::new(HashMap::new);

/// Resolves every top-level `const`, folding its initializer to a literal on
/// the way. Order matters: a `const` may only reference constants declared
/// before it, which is what keeps folding a single pass.
pub(crate) fn collect_consts(
    program: &ast::Program,
    project: &Project,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    env: &TypeEnv,
    const_fns: &ConstFns,
) -> std::result::Result<HashMap<String, ConstInfo>, Diagnostic> {
    let mut out: HashMap<String, ConstInfo> = HashMap::new();
    for decl in &program.consts {
        let prefix = project.prefix_of(&decl.module);
        let aliases = project.alias_prefixes_of(&decl.module);
        let bare = bare_name(&decl.name, prefix).to_string();
        let declared = TypeScope {
            structs,
            enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(&out),
            const_fns: Some(const_fns),
            handles: &env.handles,
        }
        .resolve(&decl.ty, false)
        .map_err(|e| e.in_module(&decl.module))?;
        if out.contains_key(&decl.name) {
            return Err(Diagnostic::new(
                format!("const '{bare}' is declared more than once"),
                decl.span,
                decl.module.clone(),
            ));
        }
        // A const shares its module's top-level namespace with `datac`s,
        // pds and functions. `resolve.rs` has already prefixed `decl.name`,
        // so a plain comparison against the prefixed names is enough.
        if structs.contains_key(&decl.name)
            || program
                .functions
                .iter()
                .any(|f| f.receiver.is_none() && f.name == decl.name)
            || program.pds.iter().any(|p| p.name == decl.name)
        {
            return Err(Diagnostic::new(
                format!("'{bare}' is used for both a const and another top-level item"),
                decl.span,
                decl.module.clone(),
            ));
        }
        let literal = {
            if let Some(result) = const_int_array(decl, &declared) {
                let elems = result.map_err(|message| Diagnostic::new(message, decl.span, decl.module.clone()))?;
                tir::Expr {
                    kind: tir::ExprKind::ArrayLit(elems),
                    ty: declared.clone(),
                    span: decl.init.span(),
                }
            } else {
                let evaluator = ConstEvaluator::new(const_fns, structs, enums, env, &out);
                evaluator
                    .fold_const(&bare, &decl.init, &declared, prefix, &aliases)
                    .map_err(|e| e.in_module(&decl.module))?
            }
        };
        out.insert(decl.name.clone(), ConstInfo { value: literal });
    }
    Ok(out)
}

/// The name a module's own source writes for a prefixed top-level item.
pub(crate) fn bare_name<'a>(qualified: &'a str, prefix: &str) -> &'a str {
    qualified.strip_prefix(prefix).unwrap_or(qualified)
}

/// Resolves and constant-folds every top-level `static` (kernel stage 7). Runs
/// before any body is checked, because a body may name a static declared
/// anywhere in its module and a `pd` initializer may read one. The returned
/// table is keyed by the module-qualified name, exactly as `consts` is; the
/// value carries the type and mutability the use site needs, and the folded
/// initializer is what the backends emit as a global constant.
pub(crate) fn collect_statics(
    program: &ast::Program,
    project: &Project,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    env: &TypeEnv,
    fns: &FuncEnv,
) -> std::result::Result<(HashMap<String, StaticInfo>, Vec<tir::Static>), Diagnostic> {
    let mut table: HashMap<String, StaticInfo> = HashMap::new();
    let mut out: Vec<tir::Static> = Vec::new();
    for decl in &program.statics {
        let prefix = project.prefix_of(&decl.module);
        let bare = bare_name(&decl.name, prefix).to_string();
        if table.contains_key(&decl.name)
            || consts.contains_key(&decl.name)
            || structs.contains_key(&decl.name)
            || sigs.keys().any(|k| k == &decl.name)
        {
            return Err(Diagnostic::new(
                format!("'{bare}' is used for both a static and another top-level item"),
                decl.span,
                decl.module.clone(),
            ));
        }
        // `percpu` is a freestanding-only storage class, and it is only
        // meaningful when mutable: one shared copy per CPU that no code may
        // write is an ordinary `static` with extra address space (kernel
        // stage 8). Both are checked here, before any backend is named, so
        // `kelpc check` and `kelpc build` refuse identically.
        if decl.percpu && !decl.mutable {
            return Err(Diagnostic::new(
                format!(
                    "a per-CPU static must be 'mut', written 'percpu static mut {bare}: T = INIT': a read-only per-CPU is just a 'static'"
                ),
                decl.span,
                decl.module.clone(),
            ));
        }
        if decl.percpu && !project.target().is_freestanding() {
            return Err(Diagnostic::new(
                format!(
                    "the per-CPU static '{bare}' is only legal in a freestanding build: one copy per CPU needs a bare-metal target whose CPU index register the kernel sets, so '[target] mode = \"freestanding\"' and a '[target] cpus' are required"
                ),
                decl.span,
                decl.module.clone(),
            ));
        }
        let aliases = project.alias_prefixes_of(&decl.module);
        let ty = TypeScope {
            structs,
            enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(consts),
            const_fns: Some(const_fns),
            handles: &env.handles,
        }
        .resolve(&decl.ty, false)
        .map_err(|e| e.in_module(&decl.module))?;
        check_static_type(&ty, structs)
            .map_err(|msg| Diagnostic::new(msg, decl.span, decl.module.clone()))?;
        // A static's initializer is checked as an ordinary expression against
        // the declared type, so a type error reads the same here as anywhere
        // else; it is then folded to a constant, and anything that is not one
        // is refused with its own message rather than silently reaching a
        // backend that cannot run code at startup. The table holds only the
        // statics declared *before* this one, so a forward reference is an
        // undefined name and a backward one is the "not constant" refusal.
        let mut ctx = base_ctx(
            &decl.module,
            sigs,
            structs,
            enums,
            consts,
            const_fns,
            owned_structs,
            overloads,
            project,
            &[],
            None,
            &HashMap::new(),
            env,
            fns,
            &table,
        );
        let value = check_expr(&decl.init, &mut ctx, Some(&ty))
            .map_err(|e| e.in_module(&decl.module))?;
        let init = static_init_of(&value, &ty, structs)
            .map_err(|msg| Diagnostic::new(msg, decl.span, decl.module.clone()))?;
        let info = StaticInfo {
            qualified: decl.name.clone(),
            name: bare,
            ty: ty.clone(),
            mutable: decl.mutable,
        };
        table.insert(decl.name.clone(), info);
        out.push(tir::Static {
            name: decl.name.clone(),
            module: decl.module.clone(),
            ty,
            mutable: decl.mutable,
            percpu: decl.percpu,
            section: decl.section.clone(),
            init,
        });
    }
    Ok((table, out))
}

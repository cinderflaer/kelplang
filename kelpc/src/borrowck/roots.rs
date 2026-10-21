use super::*;

pub(crate) fn is_copy(ty: &Ty) -> bool {
    ty.is_copy()
}

/// Whether a binding of this type is a reference whose provenance a cursor
/// carries: a shared or exclusive borrow, a `View`, a `str` or a `&dyn`.
/// `str`/`Dyn` join the borrow types here because `borrow_root` and the
/// existing `val`/`var` tracking already treat them as borrowed values.
pub(crate) fn is_reference_ty(ty: &Ty) -> bool {
    crate::infer::is_borrowed_value(ty) || matches!(ty, Ty::Str | Ty::Dyn { .. })
}

/// A root name as a person wrote it. A passdown's hidden parameter is
/// `pd#<qualified name>`, which no user ever typed, so diagnostics translate
/// it back (M6.4).
pub(crate) fn display_root(name: &str) -> String {
    match name.strip_prefix("pd#") {
        Some(qualified) => format!("passdown '{qualified}'"),
        None => match name.strip_prefix("#cap") {
            // A captured binding of a scoped block has no name of its own here.
            Some(rest) => {
                let path = rest.trim_start_matches(|c: char| c.is_ascii_digit());
                if path.is_empty() {
                    "'a captured value'".to_string()
                } else {
                    format!("'a captured value{}'", path.replace(PATH_SEP, "."))
                }
            }
            None => format!("'{}'", name.replace(PATH_SEP, ".")),
        },
    }
}

/// The separator between an owner and the fields of it a borrow is rooted at
/// (`s/items`). A character no name contains, so a module path's dots (a
/// passdown is `pd#std.rt.x`) are never read as one.
pub(crate) const PATH_SEP: char = '/';

/// The variable a root lives in: `s` for `s/items/head`.
pub(crate) fn var_of(root: &str) -> &str {
    root.split(PATH_SEP).next().unwrap_or(root)
}

/// Whether two roots can name overlapping storage: the same place, or one a
/// part of the other. Two fields of one owner neither contains the other.
pub(crate) fn roots_overlap(a: &str, b: &str) -> bool {
    a == b
        || (a.len() < b.len() && b.starts_with(a) && b.as_bytes()[a.len()] == PATH_SEP as u8)
        || (b.len() < a.len() && a.starts_with(b) && a.as_bytes()[b.len()] == PATH_SEP as u8)
}

/// One projection written the way a person reads it: `bag.xs`, or `o.text`
/// for a payload field of the enum `o` holds.
pub(crate) fn field_display(name: &str, proj: &Projection) -> String {
    match proj {
        Projection::Field(field) => format!("{name}.{field}"),
        Projection::NestedField(path) => format!("{name}.{}", path.join(".")),
        Projection::EnumPayload { name: field, .. } => format!("{name}.{field}"),
    }
}

/// The path a projection names, one entry per step. A payload field has no
/// name another projection could share, so it is keyed by the two indices its
/// `tir` node carries, behind a `#` no field name can start with: the same
/// payload moved twice then compares equal, and a payload never compares equal
/// to a `datac` field.
pub(crate) fn projection_field_path(proj: &Projection) -> Vec<String> {
    match proj {
        Projection::Field(field) => vec![field.clone()],
        Projection::NestedField(path) => path.clone(),
        Projection::EnumPayload { variant, field, .. } => vec![format!("#{variant}.{field}")],
    }
}

/// The owned binding a value read of `base.<field>` takes the field out of,
/// when `base` is a binding this frame owns *directly*. A borrow, a pointer, a
/// `pd` global's storage, and a nested place all fail here: the frame has
/// nothing to give away from any of them. A nested place is left out because a
/// projection path, not a single field name, is what tracking one would need.
/// An `enum` payload read takes the same base: `infer.rs` reads a payload by
/// value only when the scrutinee is an owned local, which this is.
pub(crate) fn partial_move_base<'a>(base: &'a tir::Expr, ctx: &Ctx) -> Option<&'a str> {
    let tir::ExprKind::Ident(name) = &base.kind else {
        return None;
    };
    if matches!(base.ty, Ty::Ref { .. }) {
        return None;
    }
    ctx.is_owned_binding(name).then_some(name.as_str())
}

/// The owned local and embedded-field path a partial move reaches. A path is
/// valid only when every link is an owned field of a local this frame owns;
/// dereferences, passdowns and borrowed bases never enter this representation.
pub(crate) fn partial_move_target<'a>(base: &'a tir::Expr, ctx: &Ctx) -> Option<(&'a str, Vec<String>)> {
    match &base.kind {
        tir::ExprKind::Ident(name) if !matches!(base.ty, Ty::Ref { .. }) && ctx.is_owned_binding(name) => {
            Some((name, Vec::new()))
        }
        tir::ExprKind::FieldAccess { base: inner, field } => {
            let (name, mut path) = partial_move_target(inner, ctx)?;
            path.push(field.clone());
            Some((name, path))
        }
        _ => None,
    }
}

/// The drops for a set of owned locals: reverse declaration order, live ones
/// only. Reverse order matches the lexical nesting of the bindings - a value
/// moved out of an earlier binding into a later one is freed exactly once,
/// by the later one, and before the earlier one would have been. "Live"
/// includes a `MaybeMoved` binding: its slot holds either the value or the
/// zero a move left there, and freeing the zero is what makes the drop correct
/// on both paths (the module doc's "a move clears its source").
pub(crate) fn live_drops<'a>(owned: impl DoubleEndedIterator<Item = &'a OwnedVar>) -> Vec<DropTarget> {
    owned
        .rev()
        .filter(|o| o.state != MoveState::Moved)
        .map(|o| DropTarget {
            binding: Some(o.binding),
            name: o.name.clone(),
            ty: o.ty.clone(),
        })
        .collect()
}

/// The root identifier a place-expression is ultimately reached through.
pub(crate) fn tir_root_ident(expr: &tir::Expr) -> Option<&str> {
    match &expr.kind {
        tir::ExprKind::Ident(name) => Some(name),
        tir::ExprKind::FieldAccess { base, .. } => tir_root_ident(base),
        tir::ExprKind::EnumField { base, .. } => tir_root_ident(base),
        tir::ExprKind::Deref { expr: inner } => tir_root_ident(inner),
        // A `pd` read in `main`/an initializer is `Deref(Ref(global))` - the
        // explicit borrow of the generated global - so the chain has to
        // descend through that `Ref` for the borrow to be tracked at all
        // (M6.4).
        tir::ExprKind::Ref { expr: inner, .. } => tir_root_ident(inner),
        // A `&dyn` coercion wraps the `&x` it was written from (A-12 slice 4),
        // so the same chain continues through it. An owned trait object
        // (A-12 slice 7) and a `&dyn` borrowed back out of one are the box's
        // own place, so the chain continues through them too.
        tir::ExprKind::DynCoerce { expr: inner, .. }
        | tir::ExprKind::BoxDynCoerce { expr: inner, .. }
        | tir::ExprKind::BoxDynBorrow { expr: inner, .. }
        | tir::ExprKind::BoxDynIsEmpty { expr: inner } => tir_root_ident(inner),
        _ => None,
    }
}

/// The `&expr`/`&mut expr` a node is a borrow of, looking through the `&dyn`
/// coercion (A-12 slice 4) that wraps one: a `&dyn` value *is* the borrow it
/// was written from, so every borrow rule applies to the node inside it.
pub(crate) fn borrowed_node(expr: &tir::Expr) -> Option<(&bool, &tir::Expr)> {
    match &expr.kind {
        tir::ExprKind::Ref { mutable, expr: inner } => Some((mutable, inner)),
        tir::ExprKind::DynCoerce { expr: inner, .. } => borrowed_node(inner),
        // `b.get()` on an owned trait object (A-12 slice 7) is a borrow of the
        // box, so a binding of it keeps the box borrowed for its own lifetime.
        tir::ExprKind::BoxDynBorrow { mutable, expr: inner } => Some((mutable, inner)),
        _ => None,
    }
}

/// The owner roots of a call argument, which is usually an auto-borrow node
/// that `check_call_arg` synthesized, so this unwraps one `Ref` before asking
/// for the argument's place roots.
pub(crate) fn arg_roots(expr: &tir::Expr, ctx: &Ctx) -> Vec<String> {
    match &expr.kind {
        tir::ExprKind::Ref { expr: inner, .. } => place_roots(inner, ctx),
        _ => place_roots(expr, ctx),
    }
}

/// Stands in for a `str` literal's storage: static, so it satisfies any
/// return lifetime and never pins an owner.
pub(crate) const STATIC_ROOT: &str = "<static>";

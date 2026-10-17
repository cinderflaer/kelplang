use super::*;

/// The name a tuple type's derived record is keyed by: the tuple as it is
/// written, element by element in position order (`(i32, i32)`).
///
/// A tuple is a record with positional fields (`design/syntax.md (kelp wiki)` 3), and v1's
/// record type is `Ty::Struct(<key>)`, the same shape A-2 gives a generic
/// instantiation (`Pair<i32, String>`). The key is the written form rather
/// than a mangled identifier because a written form cannot be a declared name:
/// `(` is not an identifier byte, so a derived tuple record can never collide
/// with a `datac`/`enum` a program declares, and `Ty::display` of the record
/// then reads exactly as the source wrote it. `ty_mangle` turns it into a C
/// identifier the way it does for every other name.
pub fn tuple_struct_name(elems: &[Ty]) -> String {
    format!(
        "({})",
        elems
            .iter()
            .map(|e| e.display())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Whether a record name is one of the compiler's derived tuple records. The
/// key is the tuple as written, so this is a property of the name alone: no
/// declared `datac`/`enum` name begins with `(`.
pub fn is_tuple_name(name: &str) -> bool {
    name.starts_with('(')
}

/// The field name one tuple position holds in its derived record. One place
/// names it, because the record's field list, the `tir` field access a
/// destructuring binding lowers to, and the `Projection` a partial move of one
/// position records all have to agree.
pub fn tuple_field_name(index: usize) -> String {
    format!("f{index}")
}

/// The position a derived record's field name stands for, or `None` for a
/// declared field. The inverse of `tuple_field_name`, so a pass that reports
/// the place a tuple element was read from names the position a reader counts
/// rather than the name the record uses.
pub fn tuple_field_index(name: &str) -> Option<usize> {
    name.strip_prefix('f')?.parse::<usize>().ok()
}

/// The `datac` record one tuple type *is*: its fields are its positions, named
/// `f0`, `f1`, ... in element order, each holding that element by value.
///
/// The declaration is derived (nothing in the source declares it), and
/// `infer.rs` registers it on first use, exactly as it registers a generic
/// instantiation (A-2). `order` is the same sentinel an instance carries:
/// `order_declarations` gives every derived record its place in the merged
/// declaration order before codegen sees it.
pub fn tuple_struct_def(name: &str, elems: &[Ty]) -> StructDef {
    StructDef {
        name: name.to_string(),
        fields: elems
            .iter()
            .enumerate()
            .map(|(i, ty)| FieldDef {
                name: tuple_field_name(i),
                ty: ty.clone(),
                mutable: false,
            })
            .collect(),
        repr: Repr::C,
        order: usize::MAX,
    }
}

/// The size and alignment of one type, in bytes, as the C compiler lays it out
/// on the x86-64 target this compiler builds for. It is what `size_of`,
/// `align_of` and `offset_of` report, and it exists so the LLVM backend and the
/// C text it writes can be checked against the same numbers rather than each
/// assuming the other agrees (kernel stage 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

pub(super) fn round_up(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

/// One field's contribution, and the offset it lands at: the C rule, with the
/// type's own `repr` deciding whether padding is present and whether the
/// alignment is raised.
pub(super) fn record_layout(
    fields: &[FieldDef],
    repr: &Repr,
    structs: &std::collections::HashMap<String, StructDef>,
    enums: &std::collections::HashMap<String, EnumDef>,
) -> Option<Layout> {
    let packed = matches!(repr, Repr::Packed);
    let mut offset = 0u64;
    let mut align = 1u64;
    for f in fields {
        let l = type_layout(&f.ty, structs, enums)?;
        if !packed {
            offset = round_up(offset, l.align);
            align = align.max(l.align);
        }
        offset += l.size;
    }
    if let Repr::Align(n) = repr {
        align = align.max(*n);
    }
    Some(Layout {
        size: round_up(offset, align),
        align,
    })
}

/// The size and alignment of one type, or `None` for a type this compiler does
/// not lay out by value (which no query may name).
pub fn type_layout(
    ty: &Ty,
    structs: &std::collections::HashMap<String, StructDef>,
    enums: &std::collections::HashMap<String, EnumDef>,
) -> Option<Layout> {
    Some(match ty {
        Ty::U8 | Ty::Bool => Layout { size: 1, align: 1 },
        Ty::U16 => Layout { size: 2, align: 2 },
        Ty::I32 | Ty::U32 | Ty::F32 | Ty::Char => Layout { size: 4, align: 4 },
        Ty::I64 | Ty::U64 | Ty::F64 | Ty::Usize | Ty::Isize => Layout { size: 8, align: 8 },
        // `unit` stores nothing; a `unit` field is filtered out of every
        // aggregate, so the pair below only ever answers a direct query.
        Ty::Unit => Layout { size: 0, align: 1 },
        // A pointer, an opaque handle and a function value are one machine
        // word; `str` is the pointer it borrows.
        Ty::Str | Ty::Handle(_) | Ty::Ptr(_) | Ty::Fn { .. } | Ty::Ref { .. } | Ty::Thread(_) => {
            Layout { size: 8, align: 8 }
        }
        // A borrowed or owned trait object is the fat pair: two pointers.
        Ty::Dyn { .. } | Ty::BoxDyn { .. } => Layout { size: 16, align: 8 },
        Ty::Mutex(_) => Layout { size: 8, align: 8 },
        Ty::RwLock(_) | Ty::Condvar => Layout { size: 8, align: 8 },
        Ty::View => Layout { size: 16, align: 8 },
        Ty::Bytes | Ty::String | Ty::List(_) | Ty::Closure { .. } | Ty::ScopedFn { .. } => {
            Layout { size: 24, align: 8 }
        }
        Ty::Box(_) => Layout { size: 8, align: 8 },
        // `{ entries, entry_count, entry_cap, len, index, index_cap, index_used }`.
        Ty::Map(_, _) => Layout { size: 56, align: 8 },
        // `{ elem v; }`, one machine word of the element.
        Ty::Atomic(elem) => type_layout(elem, structs, enums)?,
        Ty::Array(elem, len) => {
            let l = type_layout(elem, structs, enums)?;
            Layout {
                size: l.size.saturating_mul(*len),
                align: l.align,
            }
        }
        // A const argument is substituted before any layout query, so it is
        // not a type with a layout.
        Ty::ConstInt { .. } => return None,
        Ty::Struct(name) => {
            if let Some(e) = enums.get(name) {
                let (tag_bytes, _) = e.tag_storage();
                let tag = Layout {
                    size: tag_bytes,
                    align: tag_bytes,
                };
                let mut union_align = 1u64;
                let mut union_size = 0u64;
                for v in &e.variants {
                    // A payload-less variant is still one `char` in the C
                    // union (`VariantDef::is_payload_less`), so its member is
                    // one byte rather than an empty struct.
                    let p = if v.is_payload_less() {
                        Layout { size: 1, align: 1 }
                    } else {
                        record_layout(&v.fields, &Repr::C, structs, enums)?
                    };
                    union_align = union_align.max(p.align);
                    union_size = union_size.max(p.size);
                }
                let union_size = round_up(union_size, union_align);
                let union_offset = round_up(tag.size, union_align);
                let align = tag.align.max(union_align);
                Layout {
                    size: round_up(union_offset + union_size, align),
                    align,
                }
            } else {
                let s = structs.get(name)?;
                record_layout(&s.fields, &s.repr, structs, enums)?
            }
        }
    })
}

/// The byte offset of one field of one `datac`, or `None` when the type is not
/// a `datac` this compiler lays out or the field is not one of its own.
pub fn field_offset(
    name: &str,
    field: &str,
    structs: &std::collections::HashMap<String, StructDef>,
    enums: &std::collections::HashMap<String, EnumDef>,
) -> Option<u64> {
    let s = structs.get(name)?;
    if enums.contains_key(name) {
        return None;
    }
    let packed = matches!(s.repr, Repr::Packed);
    let mut offset = 0u64;
    for f in &s.fields {
        let l = type_layout(&f.ty, structs, enums)?;
        if !packed {
            offset = round_up(offset, l.align);
        }
        if f.name == field {
            return Some(offset);
        }
        offset += l.size;
    }
    None
}

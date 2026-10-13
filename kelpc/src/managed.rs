//! KelpScript managed mode, front end (WP2 of the collector design,
//! `kelpscript-gc-paper.md` sections 7 and 10).
//!
//! Managed mode keeps memory values in a collected heap and refuses resource
//! values there. This module decides which is which, and it is the only place
//! that decides it. The checker asks `resolve` (or `resource_kind` when no
//! program is in hand) and then `check_heap_slot` at each position a value
//! could enter the heap: a `Box` payload, a `List` or `Map` element, an array
//! element, a field of a heap-resident `datac`, or a captured closure value.
//!
//! Nothing here generates code. The collector and the LLVM lowering are other
//! work packages and read nothing from this file except the mode they are
//! gated on.

#![cfg_attr(not(test), allow(dead_code))]

use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::types::Ty;

/// The heap cap, in MiB, a managed program gets when its manifest names no
/// `heap_max` (`kelpscript-gc-paper.md` section 6.1). Chosen so a managed
/// program cannot take the host's memory from the Minecraft server.
pub const DEFAULT_HEAP_MAX_MIB: u64 = 256;

/// The first clearing's start guesses and the headroom percent, when the
/// manifest's `[tidepool]` section names none (design section 5, D2).
pub const DEFAULT_START_LOW_MIB: u64 = 1;
pub const DEFAULT_START_HIGH_MIB: u64 = 4;
pub const DEFAULT_HEADROOM_PCT: u64 = 10;

/// The values `[target] heap_max` accepts, in MiB. The top bound stops a typo
/// from asking the kernel for a heap no 7.6 GB host can give.
pub const HEAP_MAX_MIB_RANGE: std::ops::RangeInclusive<u64> = 1..=65_536;

/// Whether a type's values may live in the collected heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// Holds only GC memory, or nothing that owns anything. It may enter the
    /// heap.
    Memory,
    /// Owns a runtime record, a native handle or a capability. It never enters
    /// the heap, and its drop runs at scope end.
    Resource,
    /// A user declaration (a `datac` or `enum` outside the golden tables). Its
    /// answer depends on the declaration (a user destructor, or a resource
    /// field), which `resolve` reads. `resource_kind` alone cannot answer it.
    Declared,
}

/// What a user declaration says about itself, for `resolve`. `fields` lists
/// the type of every stored field, and for an `enum` every payload field of
/// every variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclFacts {
    pub has_user_drop: bool,
    pub fields: Vec<Ty>,
}

/// The stdlib `datac`s and `enum`s that are resources, by canonical key
/// (`std.net.Socket`), as `Ty::Struct` carries them. Three groups:
/// types with a destructor, types that hold a file or process descriptor
/// (the same set `stdlib::handle_ctor_domains` names), and capability receipts
/// (`stdlib::SEALED_TYPES`). `kelpscript-gc-paper.md` section 7.1 calls the
/// descriptor holders memory values; the decided defaults of this work package
/// refuse them, and the report records the difference.
pub const STDLIB_RESOURCE_STRUCTS: &[&str] = &[
    "std.pool.Pool",
    "std.sync.SendEnd",
    "std.sync.RecvEnd",
    "std.sync.Sender",
    "std.sync.Receiver",
    "std.io.File",
    "std.net.Socket",
    "std.net.Listener",
    "std.process.Child",
    "std.ws.Conn",
    "std.tls.TlsStream",
    "std.http.Conn",
    "std.http.Lease",
    "std.http.Cursor",
    "std.http.Server",
    "std.http.ServerConn",
    "std.http.Pool",
    "std.irq.Line",
    "std.frame.Frame",
];

/// Stdlib `datac`s and `enum`s that hold only memory, by canonical key. They
/// are golden so a stdlib type that starts holding a handle shows up as a
/// test failure, not a silent change.
pub const STDLIB_MEMORY_STRUCTS: &[&str] = &[
    "std.http.Url",
    "std.http.Header",
    "std.http.Request",
    "std.http.Response",
    "std.http.ServerResponse",
    "std.fs.Metadata",
    "std.tls.TlsOptions",
    "std.sync.ChanMeta",
];

/// The kind of a value a checker is about to place in the heap. Used in
/// messages, so a refusal names the position as well as the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapSlot {
    BoxPayload,
    ListElement,
    MapKey,
    MapValue,
    ArrayElement,
    DatacField,
    ClosureCapture,
    /// The argument a `thread.spawn` moves into its worker. It sits in a
    /// malloc'd start record the collector does not scan (Tidepool, step 0).
    ThreadArgument,
    /// The value a worker returns through `join`. It sits in a malloc'd slot
    /// the collector does not scan.
    ThreadResult,
    /// The value a `Mutex` or `RwLock` holds. The record is malloc'd, so the
    /// collector does not scan it.
    LockValue,
}

impl HeapSlot {
    fn describe(self) -> &'static str {
        match self {
            HeapSlot::BoxPayload => "the payload of a Box",
            HeapSlot::ListElement => "a List element",
            HeapSlot::MapKey => "a Map key",
            HeapSlot::MapValue => "a Map value",
            HeapSlot::ArrayElement => "an array element",
            HeapSlot::DatacField => "a field of a heap-resident datac",
            HeapSlot::ClosureCapture => "a value captured by a closure environment",
            HeapSlot::ThreadArgument => "a thread's argument",
            HeapSlot::ThreadResult => "a thread's result",
            HeapSlot::LockValue => "the value of a Mutex or RwLock",
        }
    }
}

/// How two parts of one type combine: a container is a resource when any part
/// is, and undecided when any part is undecided and none is a resource.
fn join(a: ResourceKind, b: ResourceKind) -> ResourceKind {
    use ResourceKind::*;
    match (a, b) {
        (Resource, _) | (_, Resource) => Resource,
        (Declared, _) | (_, Declared) => Declared,
        _ => Memory,
    }
}

/// The classifier's walk state: the user declarations it may consult, and the
/// declared types on the current path, so a recursive `datac` through a `Box`
/// ends the walk instead of looping.
struct Walk<'a> {
    decls: &'a dyn Fn(&str) -> Option<DeclFacts>,
    path: Vec<String>,
}

fn classify(ty: &Ty, walk: &mut Walk<'_>) -> ResourceKind {
    use ResourceKind::*;
    match ty {
        // Scalars and the `str` pointer: no owned value, nothing to collect.
        Ty::I32
        | Ty::I64
        | Ty::U32
        | Ty::U64
        | Ty::F32
        | Ty::F64
        | Ty::Bool
        | Ty::Char
        | Ty::Unit
        | Ty::Str
        | Ty::U8
        | Ty::U16
        | Ty::Usize
        | Ty::Isize => Memory,
        // GC byte buffers and the length-carrying text view. A `View` is an
        // interior pointer, and it is refused as a field, element and key by the
        // language itself (`plan/syntax.md` 11.5), not by this classifier.
        Ty::Bytes | Ty::String | Ty::View => Memory,
        // A user or stdlib declaration. A golden name answers at once; any other
        // name is read from its declaration.
        Ty::Struct(name) => struct_kind(name, walk),
        // Containers: a container holds memory exactly when its elements do.
        Ty::List(elem) | Ty::Box(elem) | Ty::Array(elem, _) => classify(elem, walk),
        Ty::Map(key, value) => {
            let k = classify(key, walk);
            let v = classify(value, walk);
            join(k, v)
        }
        // Scalars only (`is_atomic_element`), so no record and no pointer.
        Ty::Atomic(_) => Memory,
        // Runtime records the C side owns and frees: `kelp_rt.c` thread, mutex,
        // rwlock and condition-variable records. Their drop is a deterministic
        // runtime call, so they are resources whatever their element type.
        Ty::Thread(_) | Ty::Mutex(_) | Ty::RwLock(_) | Ty::Condvar => Resource,
        // An `extern type` is an opaque native handle (`TlsHandle`, `TlsCtx`). The
        // collector never closes it, so it stays scope-owned.
        Ty::Handle(_) => Resource,
        // A raw address owns nothing and is outside the collector's contract
        // (paper section 7.2, rule 7). It is not a resource, and it does not keep
        // anything alive.
        Ty::Ptr(_) => Memory,
        // Borrows own nothing. The value they point at is classified where it is
        // owned, so a borrow is a pointer and never a heap owner.
        Ty::Ref { .. } | Ty::Dyn { .. } => Memory,
        // A trait object's concrete type is known only where it is coerced into
        // the box, so the program's declarations decide it.
        Ty::BoxDyn { .. } => Declared,
        // A function value is a code pointer. A closure owns a heap environment,
        // which the collector traces. Its captured values are checked at capture.
        Ty::Fn { .. } | Ty::ScopedFn { .. } | Ty::Closure { .. } => Memory,
        // Exists only inside an instantiation; a scalar value when it appears.
        Ty::ConstInt { .. } => Memory,
    }
}

fn struct_kind(name: &str, walk: &mut Walk<'_>) -> ResourceKind {
    use ResourceKind::*;
    if STDLIB_RESOURCE_STRUCTS.contains(&name) {
        return Resource;
    }
    if STDLIB_MEMORY_STRUCTS.contains(&name) {
        return Memory;
    }
    let Some(facts) = (walk.decls)(name) else {
        return Declared;
    };
    // A destructor makes a declaration a resource: its drop must run at scope
    // end, which the collector cannot promise (paper section 11, question 3).
    if facts.has_user_drop {
        return Resource;
    }
    // A type already on the path is the recursion, not a new part: it adds no
    // resource that its first visit does not already count.
    if walk.path.iter().any(|p| p == name) {
        return Memory;
    }
    walk.path.push(name.to_string());
    let kind = facts
        .fields
        .iter()
        .fold(Memory, |acc, field| join(acc, classify(field, walk)));
    walk.path.pop();
    kind
}

/// The classification of a type with no program in hand. Every user `datac` or
/// `enum` comes back `Declared`, and only the golden stdlib tables and the
/// type's own shape decide the rest. The checker calls `resolve` instead,
/// which reads the declarations.
pub fn resource_kind(ty: &Ty) -> ResourceKind {
    let none = |_: &str| -> Option<DeclFacts> { None };
    let mut walk = Walk { decls: &none, path: Vec::new() };
    classify(ty, &mut walk)
}

/// The classification of a type given the program's declarations. A user
/// declaration with a destructor is a resource, and so is one with a resource
/// field. A declaration that `decls` does not know stays `Declared`.
pub fn resolve(ty: &Ty, decls: &dyn Fn(&str) -> Option<DeclFacts>) -> ResourceKind {
    let mut walk = Walk { decls, path: Vec::new() };
    classify(ty, &mut walk)
}

/// Refuses a value of type `ty_name` going into the heap at `slot`, unless it is
/// memory. A resource is refused by name, and so is a type whose kind is still
/// undecided, because an undecided type is never quietly let in.
pub fn check_heap_slot(
    slot: HeapSlot,
    ty_name: &str,
    kind: ResourceKind,
    at: Span,
) -> Result<(), Diagnostic> {
    match kind {
        ResourceKind::Memory => Ok(()),
        ResourceKind::Resource => Err(Diagnostic::new(
            format!(
                "'{ty_name}' cannot be {} in a hosted build: '{ty_name}' is a resource, and a resource never enters the collected heap, because its drop has to run at scope end. Keep it in a local, a parameter or a field of a stack value, and let it drop there",
                slot.describe()
            ),
            at,
            "",
        )),
        ResourceKind::Declared => Err(Diagnostic::new(
            format!(
                "'{ty_name}' cannot be {} in a hosted build: its declaration was not found, so it cannot be classified as memory",
                slot.describe()
            ),
            at,
            "",
        )),
    }
}

/// One value the checker saw go into a heap slot. The checker records these
/// while it runs and `check_heap_uses` decides them after the pass, when every
/// declaration's facts are known (a `datac` may name one declared later in the
/// same program, which a check made mid-pass could not see).
#[derive(Debug, Clone)]
pub struct HeapUse {
    pub slot: HeapSlot,
    pub ty: Ty,
    pub span: Span,
}

/// The facts of the program's own `datac` or `enum` called `name`, read from
/// the checked declarations. `None` for a name the program does not declare.
pub fn decl_facts(
    structs: &[crate::types::StructDef],
    enums: &[crate::types::EnumDef],
    drops: &std::collections::HashSet<String>,
    name: &str,
) -> Option<DeclFacts> {
    let has_user_drop = drops.contains(name);
    // An enum is also listed among the structs, with no fields, so the enum
    // table is read first.
    if let Some(e) = enums.iter().find(|e| e.name == name) {
        return Some(DeclFacts {
            has_user_drop,
            fields: e
                .variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                .collect(),
        });
    }
    let s = structs.iter().find(|s| s.name == name)?;
    Some(DeclFacts {
        has_user_drop,
        fields: s.fields.iter().map(|f| f.ty.clone()).collect(),
    })
}

/// Decides every recorded heap use against the program's declarations, and
/// returns the first refusal in source order. `decls` answers for each user
/// `datac` or `enum` by canonical name.
pub fn check_heap_uses(
    uses: &[HeapUse],
    decls: &dyn Fn(&str) -> Option<DeclFacts>,
) -> Result<(), Diagnostic> {
    let mut ordered: Vec<&HeapUse> = uses.iter().collect();
    ordered.sort_by_key(|u| u.span.start);
    for u in ordered {
        let kind = resolve(&u.ty, decls);
        check_heap_slot(u.slot, &u.ty.display(), kind, u.span)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span::new(0, 0)
    }

    fn list(t: Ty) -> Ty {
        Ty::List(Box::new(t))
    }

    fn map(k: Ty, v: Ty) -> Ty {
        Ty::Map(Box::new(k), Box::new(v))
    }

    /// One value of every `Ty` variant, each with the kind section 7 gives it.
    /// `resource_kind` matches exhaustively, so a new variant is a compile error
    /// there; this list is what a reviewer reads to see the answer for each.
    fn every_variant() -> Vec<(&'static str, Ty, ResourceKind)> {
        use ResourceKind::*;
        vec![
            ("I32", Ty::I32, Memory),
            ("I64", Ty::I64, Memory),
            ("U32", Ty::U32, Memory),
            ("U64", Ty::U64, Memory),
            ("F32", Ty::F32, Memory),
            ("F64", Ty::F64, Memory),
            ("Bool", Ty::Bool, Memory),
            ("Char", Ty::Char, Memory),
            ("Unit", Ty::Unit, Memory),
            ("Str", Ty::Str, Memory),
            ("U8", Ty::U8, Memory),
            ("U16", Ty::U16, Memory),
            ("Usize", Ty::Usize, Memory),
            ("Isize", Ty::Isize, Memory),
            ("Bytes", Ty::Bytes, Memory),
            ("String", Ty::String, Memory),
            ("View", Ty::View, Memory),
            ("Struct(user)", Ty::Struct("Point".into()), Declared),
            ("Struct(Socket)", Ty::Struct("std.net.Socket".into()), Resource),
            ("List(I32)", list(Ty::I32), Memory),
            ("List(Thread)", list(Ty::Thread(Box::new(Ty::I32))), Resource),
            ("Box(String)", Ty::Box(Box::new(Ty::String)), Memory),
            ("Box(Mutex)", Ty::Box(Box::new(Ty::Mutex(Box::new(Ty::I32)))), Resource),
            ("Map(String,I64)", map(Ty::String, Ty::I64), Memory),
            ("Map(String,Socket)", map(Ty::String, Ty::Struct("std.net.Socket".into())), Resource),
            (
                "Ref(&String)",
                Ty::Ref { mutable: false, inner: Box::new(Ty::String) },
                Memory,
            ),
            (
                "Ref(&mut Socket)",
                Ty::Ref { mutable: true, inner: Box::new(Ty::Struct("std.net.Socket".into())) },
                Memory,
            ),
            ("Dyn", Ty::Dyn { mutable: false, trait_name: "Name".into() }, Memory),
            ("BoxDyn", Ty::BoxDyn { trait_name: "Name".into() }, Declared),
            ("Fn", Ty::Fn { params: vec![], ret: Box::new(Ty::Unit) }, Memory),
            ("Closure", Ty::Closure { params: vec![], ret: Box::new(Ty::Unit) }, Memory),
            ("ScopedFn", Ty::ScopedFn { params: vec![], ret: Box::new(Ty::Unit) }, Memory),
            ("Array(I32,4)", Ty::Array(Box::new(Ty::I32), 4), Memory),
            (
                "Array(Socket,2)",
                Ty::Array(Box::new(Ty::Struct("std.net.Socket".into())), 2),
                Resource,
            ),
            ("ConstInt", Ty::ConstInt { value: 16, ty: Box::new(Ty::Usize) }, Memory),
            ("Atomic(I64)", Ty::Atomic(Box::new(Ty::I64)), Memory),
            ("Thread", Ty::Thread(Box::new(Ty::String)), Resource),
            ("Mutex", Ty::Mutex(Box::new(Ty::I64)), Resource),
            ("RwLock", Ty::RwLock(Box::new(Ty::I64)), Resource),
            ("Condvar", Ty::Condvar, Resource),
            ("Handle(TlsHandle)", Ty::Handle("std.tls.TlsHandle".into()), Resource),
            ("Ptr(u8)", Ty::Ptr(Box::new(Ty::U8)), Memory),
        ]
    }

    #[test]
    fn every_ty_variant_is_classified_as_listed() {
        let cases = every_variant();
        // The rows cover every top-level variant. Several rows may share one
        // variant (a `List` of memory and a `List` of a resource), so the check
        // is on distinct discriminants, not on the row count: a variant added to
        // the enum without a row here makes the set one short.
        let covered: std::collections::HashSet<_> =
            cases.iter().map(|(_, ty, _)| std::mem::discriminant(ty)).collect();
        assert_eq!(covered.len(), 36, "the Ty enum has 36 variants; every one needs a row");
        for (name, ty, want) in cases {
            assert_eq!(resource_kind(&ty), want, "classification of {name}");
        }
    }

    #[test]
    fn a_user_struct_without_a_program_is_declared_not_memory() {
        assert_eq!(resource_kind(&Ty::Struct("Node".into())), ResourceKind::Declared);
    }

    #[test]
    fn a_container_takes_the_kind_of_its_element() {
        assert_eq!(resource_kind(&list(Ty::Struct("Node".into()))), ResourceKind::Declared);
        assert_eq!(
            resource_kind(&Ty::Box(Box::new(Ty::Condvar))),
            ResourceKind::Resource
        );
        assert_eq!(
            resource_kind(&map(Ty::String, Ty::Thread(Box::new(Ty::I32)))),
            ResourceKind::Resource
        );
    }

    #[test]
    fn the_golden_stdlib_resource_types_are_resources() {
        let resource = [
            "std.pool.Pool",
            "std.sync.SendEnd",
            "std.sync.RecvEnd",
            "std.io.File",
            "std.net.Socket",
            "std.net.Listener",
            "std.tls.TlsStream",
            "std.http.Conn",
            "std.http.Lease",
            "std.http.Pool",
            "std.frame.Frame",
            "std.irq.Line",
        ];
        for key in resource {
            assert_eq!(
                resource_kind(&Ty::Struct(key.into())),
                ResourceKind::Resource,
                "{key} is a golden resource"
            );
        }
    }

    #[test]
    fn the_golden_stdlib_memory_types_are_memory() {
        for key in STDLIB_MEMORY_STRUCTS {
            assert_eq!(
                resource_kind(&Ty::Struct((*key).into())),
                ResourceKind::Memory,
                "{key} is a golden memory type"
            );
        }
    }

    /// The golden tables name real stdlib declarations. A stdlib type renamed or
    /// removed, or moved to another module, fails here rather than drifting.
    #[test]
    fn the_golden_tables_name_real_stdlib_declarations() {
        let sources: &[(&str, &str)] = &[
            ("pool", include_str!("../std/pool.kp")),
            ("sync", include_str!("../std/sync.kp")),
            ("io", include_str!("../std/io.kp")),
            ("net", include_str!("../std/net.kp")),
            ("tls", include_str!("../std/tls.kp")),
            ("http", include_str!("../std/http.kp")),
            ("ws", include_str!("../std/ws.kp")),
            ("process", include_str!("../std/process.kp")),
            ("frame", include_str!("../std/frame.kp")),
            ("irq", include_str!("../std/irq.kp")),
            ("fs", include_str!("../std/fs.kp")),
        ];
        let all: Vec<&str> = STDLIB_RESOURCE_STRUCTS
            .iter()
            .chain(STDLIB_MEMORY_STRUCTS.iter())
            .copied()
            .collect();
        for key in all {
            let (module, name) = key.rsplit_once('.').expect("a canonical key");
            let module = module.strip_prefix("std.").expect("a stdlib key");
            let text = sources
                .iter()
                .find(|(m, _)| *m == module)
                .unwrap_or_else(|| panic!("no source for module {module} of {key}"))
                .1;
            let declared = [
                format!("datac {name}("),
                format!("datac {name}<"),
                format!("pub datac {name}("),
                format!("pub datac {name}<"),
                format!("enum {name}"),
                format!("pub enum {name}"),
            ]
            .iter()
            .any(|needle| text.contains(needle.as_str()));
            assert!(declared, "{key} is not declared in std/{module}.kp");
        }
    }

    #[test]
    fn a_user_destructor_makes_a_declaration_a_resource() {
        let decls = |name: &str| {
            (name == "Guard").then(|| DeclFacts { has_user_drop: true, fields: vec![Ty::I32] })
        };
        assert_eq!(
            resolve(&Ty::Struct("Guard".into()), &decls),
            ResourceKind::Resource
        );
    }

    #[test]
    fn a_resource_field_makes_a_declaration_a_resource() {
        let decls = |name: &str| match name {
            "Holder" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![Ty::I64, Ty::Struct("std.net.Socket".into())],
            }),
            _ => None,
        };
        assert_eq!(
            resolve(&Ty::Struct("Holder".into()), &decls),
            ResourceKind::Resource
        );
    }

    #[test]
    fn a_recursive_memory_type_resolves_to_memory() {
        // datac Node(val left: Box<Node>, val right: Box<Node>)
        let decls = |name: &str| match name {
            "Node" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![
                    Ty::Box(Box::new(Ty::Struct("Node".into()))),
                    Ty::Box(Box::new(Ty::Struct("Node".into()))),
                ],
            }),
            _ => None,
        };
        assert_eq!(
            resolve(&Ty::Struct("Node".into()), &decls),
            ResourceKind::Memory
        );
    }

    #[test]
    fn a_resource_reached_through_a_recursive_type_is_still_found() {
        // datac Tree(val kids: List<Tree>, val lock: Mutex<i64>)
        let decls = |name: &str| match name {
            "Tree" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![
                    list(Ty::Struct("Tree".into())),
                    Ty::Mutex(Box::new(Ty::I64)),
                ],
            }),
            _ => None,
        };
        assert_eq!(
            resolve(&Ty::Struct("Tree".into()), &decls),
            ResourceKind::Resource
        );
    }

    #[test]
    fn an_unknown_declaration_stays_declared() {
        let decls = |_: &str| -> Option<DeclFacts> { None };
        assert_eq!(
            resolve(&Ty::Struct("Missing".into()), &decls),
            ResourceKind::Declared
        );
    }

    #[test]
    fn a_memory_value_may_enter_every_heap_slot() {
        for slot in [
            HeapSlot::BoxPayload,
            HeapSlot::ListElement,
            HeapSlot::MapKey,
            HeapSlot::MapValue,
            HeapSlot::ArrayElement,
            HeapSlot::DatacField,
            HeapSlot::ClosureCapture,
        ] {
            assert!(check_heap_slot(slot, "String", ResourceKind::Memory, span()).is_ok());
        }
    }

    #[test]
    fn a_resource_is_refused_in_the_heap_by_name() {
        let err = check_heap_slot(
            HeapSlot::BoxPayload,
            "std.net.Socket",
            ResourceKind::Resource,
            span(),
        )
        .expect_err("a resource must be refused");
        assert!(err.message.contains("'std.net.Socket' is a resource"), "{}", err.message);
        assert!(err.message.contains("the payload of a Box"), "{}", err.message);
        assert!(err.message.contains("a hosted build"), "{}", err.message);
    }

    /// The sizes the root test needs: scalars and the container headers.
    struct RootLayout;

    impl crate::gc_descr::GcLayout for RootLayout {
        fn struct_fields(&self, name: &str) -> Result<Vec<Ty>, String> {
            Err(format!("no fields for {name} in this test"))
        }
        fn enum_variants(&self, _name: &str) -> Option<Vec<Vec<Ty>>> {
            None
        }
        fn size_align(&self, ty: &Ty) -> Result<(u64, u64), String> {
            Ok(match ty {
                Ty::I32 => (4, 4),
                Ty::I64 => (8, 8),
                Ty::String | Ty::Bytes | Ty::List(_) => (24, 8),
                other => return Err(format!("no layout for {other:?}")),
            })
        }
    }

    #[test]
    fn a_value_holding_collected_memory_is_rooted_by_its_thread_or_lock_record() {
        use crate::gc_descr::record_needs_root;
        let layout = RootLayout;
        // A scalar holds no collected memory, so its record registers no root.
        assert!(!record_needs_root(&layout, &Ty::I64).unwrap());
        // A handle owns no collected slot of its own; its record roots its own value.
        assert!(!record_needs_root(&layout, &Ty::Thread(Box::new(Ty::I32))).unwrap());
        // A String or a List is collected memory, so the record that holds it registers a root.
        assert!(record_needs_root(&layout, &Ty::String).unwrap());
        assert!(record_needs_root(&layout, &list(Ty::I32)).unwrap());
    }

    #[test]
    fn an_undecided_type_is_refused_not_admitted() {
        let err = check_heap_slot(
            HeapSlot::ListElement,
            "Missing",
            ResourceKind::Declared,
            span(),
        )
        .expect_err("an undecided type must not be let in");
        assert!(err.message.contains("'Missing' cannot be a List element"), "{}", err.message);
    }
}

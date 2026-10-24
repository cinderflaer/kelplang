use super::*;

fn span() -> Span {
    Span::new(0, 0)
}

/// The trait-object answers of a program whose coercions are `coerced`, for a
/// lookup under the key [`trait_object_key`] builds.
fn dyn_decls(name: &str, coerced: &[(String, Option<Ty>)]) -> Option<DeclFacts> {
    name.strip_prefix("dyn ").and_then(|t| trait_object_facts(t, coerced))
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
        ("pool", include_str!("../../std/pool.kp")),
        ("sync", include_str!("../../std/sync.kp")),
        ("io", include_str!("../../std/io.kp")),
        ("net", include_str!("../../std/net.kp")),
        ("tls", include_str!("../../std/tls.kp")),
        ("http", include_str!("../../std/http.kp")),
        ("ws", include_str!("../../std/ws.kp")),
        ("process", include_str!("../../std/process.kp")),
        ("frame", include_str!("../../std/frame.kp")),
        ("irq", include_str!("../../std/irq.kp")),
        ("fs", include_str!("../../std/fs.kp")),
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

/// The collector keeps a fallback headroom for a program that reaches it
/// before `kelp_gc_init` (`kelpc/gc/src/state.rs`). It cannot import this
/// crate, so this reads that constant and holds it to the manifest default.
#[test]
fn the_collector_default_headroom_is_the_manifest_default() {
    let state = include_str!("../../gc/src/state.rs");
    let value = state
        .lines()
        .find_map(|line| line.strip_prefix("pub const DEFAULT_HEADROOM_PCT: usize = "))
        .and_then(|rest| rest.strip_suffix(';'))
        .expect("state.rs declares DEFAULT_HEADROOM_PCT as a usize literal");
    assert_eq!(value.parse::<u64>().expect("a whole number"), DEFAULT_HEADROOM_PCT);
}

/// The collector's heap cap, start guesses and minimum heap are written as
/// `N << 20` (N MiB) in `kelpc/gc/src/state.rs`. Each one with a manifest
/// default here must agree with it. The minimum heap has no manifest default,
/// so it is held to the copy the lowering emits (`gc_descr::DEFAULT_HEAP_MIN`).
#[test]
fn the_collector_default_heap_and_start_sizes_are_the_manifest_defaults() {
    let state = include_str!("../../gc/src/state.rs");
    let mib = |name: &str| -> u64 {
        let prefix = format!("pub const {name}: usize = ");
        let value = state
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str()))
            .and_then(|rest| rest.strip_suffix(';'))
            .unwrap_or_else(|| panic!("state.rs declares {name} as a usize"));
        let (count, shift) = value.split_once(" << ").expect("a count shifted left");
        assert_eq!(shift.parse::<u32>().expect("a whole number"), 20, "{name} is in MiB");
        count.parse::<u64>().expect("a whole number")
    };
    assert_eq!(mib("DEFAULT_HEAP_MAX"), DEFAULT_HEAP_MAX_MIB);
    assert_eq!(mib("DEFAULT_START_LOW"), DEFAULT_START_LOW_MIB);
    assert_eq!(mib("DEFAULT_START_HIGH"), DEFAULT_START_HIGH_MIB);
    assert_eq!(mib("DEFAULT_HEAP_MIN") << 20, crate::gc_descr::DEFAULT_HEAP_MIN);
}

/// The crossing table (channels plan item 2): what a worker may take as its
/// argument or return. Channel records cross with their element; a destructor,
/// a file, a pool and a container of resources do not.
#[test]
fn the_crossing_table_lifts_channel_records_and_keeps_the_rest_refused() {
    let decls = |name: &str| -> Option<DeclFacts> {
        match name {
            "std.sync.Sender<i64>" | "std.sync.Sender<bool>" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![Ty::Struct("std.sync.SendEnd".into())],
            }),
            "std.sync.Receiver<i64>" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![Ty::Struct("std.sync.RecvEnd".into())],
            }),
            "std.sync.SendEnd" | "std.sync.RecvEnd" => Some(DeclFacts {
                has_user_drop: true,
                fields: vec![Ty::I64],
            }),
            "Job" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![
                    Ty::Struct("std.sync.Sender<i64>".into()),
                    Ty::Struct("std.sync.Sender<bool>".into()),
                ],
            }),
            "Guard" => Some(DeclFacts {
                has_user_drop: true,
                fields: vec![Ty::I32],
            }),
            _ => None,
        }
    };
    let sender = || Ty::Struct("std.sync.Sender<i64>".into());
    let crosses_ty = [
        ("Sender<i64>", sender()),
        ("Receiver<i64>", Ty::Struct("std.sync.Receiver<i64>".into())),
        ("Job", Ty::Struct("Job".into())),
        ("Mutex<List<String>>", Ty::Mutex(Box::new(list(Ty::String)))),
        ("Condvar", Ty::Condvar),
        ("Thread<i32>", Ty::Thread(Box::new(Ty::I32))),
        ("i64", Ty::I64),
    ];
    for (label, ty) in crosses_ty {
        assert_eq!(
            crossing_kind(&ty, &decls),
            ResourceKind::Memory,
            "{label} should cross"
        );
    }
    let refused = [
        ("Mutex<Sender<i64>>", Ty::Mutex(Box::new(sender()))),
        ("Guard", Ty::Struct("Guard".into())),
        ("std.io.File", Ty::Struct("std.io.File".into())),
        ("std.pool.Pool", Ty::Struct("std.pool.Pool".into())),
        ("List<Sender<i64>>", list(sender())),
        ("Missing", Ty::Struct("Missing".into())),
        ("closure fun() => i64", Ty::Closure { params: vec![], ret: Box::new(Ty::I64) }),
    ];
    for (label, ty) in refused {
        assert_ne!(
            crossing_kind(&ty, &decls),
            ResourceKind::Memory,
            "{label} should stay refused"
        );
    }
}

/// Issue 233: a user destructor whose fields cross is let in as a thread
/// argument only. A closure environment and a thread result keep the rule of
/// decision 21, so the same type is still refused there.
#[test]
fn a_user_destructor_crosses_as_a_thread_argument_only() {
    let decls = |name: &str| -> Option<DeclFacts> {
        match name {
            "Guard" => Some(DeclFacts { has_user_drop: true, fields: vec![Ty::I32] }),
            "Holder" => Some(DeclFacts { has_user_drop: true, fields: vec![Ty::String] }),
            _ => None,
        }
    };
    let guard = Ty::Struct("Guard".into());
    assert_eq!(thread_argument_kind(&guard, &decls), ResourceKind::Memory);
    assert_eq!(crossing_kind(&guard, &decls), ResourceKind::Resource);
    // A destructor over a String field crosses as a String does: the worker
    // owns the value, and the start record holds it the way it holds a String.
    assert_eq!(thread_argument_kind(&Ty::Struct("Holder".into()), &decls), ResourceKind::Memory);
    assert_eq!(crossing_kind(&Ty::Struct("Holder".into()), &decls), ResourceKind::Resource);
}

/// A value a direct `thread.spawn` literal captures (decision 127) takes the
/// crossing rule that a thread argument takes: a channel end, a Mutex of memory,
/// a Condvar and the pool's shared state cross; a destructor, a file, a pool
/// handle and a thread handle of a destructor do not.
/// The pool hands a `Job` (a datac whose field is a closure) to its worker, so a
/// closure inside a crossing declaration still crosses, while a bare closure value
/// is refused (decision 131).
#[test]
fn a_closure_field_inside_a_declaration_still_crosses_but_a_bare_closure_does_not() {
    let decls = |name: &str| -> Option<DeclFacts> {
        match name {
            "Job" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![Ty::Closure { params: vec![], ret: Box::new(Ty::Unit) }],
            }),
            _ => None,
        }
    };
    assert_eq!(
        crossing_kind(&Ty::Struct("Job".into()), &decls),
        ResourceKind::Memory
    );
    assert_eq!(
        crossing_kind(&Ty::Closure { params: vec![], ret: Box::new(Ty::Unit) }, &decls),
        ResourceKind::Resource
    );
}

#[test]
fn a_spawn_entry_capture_takes_the_crossing_rule() {
    let decls = |name: &str| -> Option<DeclFacts> {
        match name {
            "std.sync.Sender<i64>" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![Ty::Struct("std.sync.SendEnd".into())],
            }),
            "std.sync.SendEnd" => Some(DeclFacts {
                has_user_drop: true,
                fields: vec![Ty::I64],
            }),
            "std.pool.PoolShared" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![
                    Ty::Mutex(Box::new(Ty::I64)),
                    Ty::Mutex(Box::new(list(Ty::Struct("Job".into())))),
                    Ty::Condvar,
                    Ty::Condvar,
                ],
            }),
            "Job" => Some(DeclFacts {
                has_user_drop: false,
                fields: vec![Ty::Closure { params: vec![], ret: Box::new(Ty::Unit) }],
            }),
            "Guard" => Some(DeclFacts {
                has_user_drop: true,
                fields: vec![Ty::I32],
            }),
            _ => None,
        }
    };
    let captured = |ty: Ty| HeapUse {
        slot: HeapSlot::SpawnEntryCapture,
        ty,
        span: span(),
    };
    let crosses_ty = [
        Ty::Struct("std.sync.Sender<i64>".into()),
        Ty::Mutex(Box::new(list(Ty::String))),
        Ty::Condvar,
        Ty::Struct("std.pool.PoolShared".into()),
    ];
    for ty in crosses_ty {
        let label = ty.display();
        assert!(
            check_heap_uses(&[captured(ty)], &decls).is_ok(),
            "{label} should be capturable by a spawn entry"
        );
    }
    let refused_ty = [
        Ty::Struct("Guard".into()),
        Ty::Struct("std.pool.Pool".into()),
        Ty::Struct("std.io.File".into()),
        Ty::Thread(Box::new(Ty::Struct("Guard".into()))),
    ];
    for ty in refused_ty {
        let label = ty.display();
        let err = check_heap_uses(&[captured(ty)], &decls)
            .expect_err("a resource must stay refused as a spawn entry capture");
        assert!(
            err.message.contains("a value captured by a closure environment"),
            "{label}: {}",
            err.message
        );
    }
}

/// A `Box<dyn Shape>` is judged by the types the program coerces into `Shape`:
/// a box of memory implementors is memory, so it may sit in a `List` (the
/// classifier gap behind `fixture_traits_boxdyn_list`).
#[test]
fn a_box_of_a_trait_is_memory_when_every_coerced_implementor_is() {
    let coerced = vec![
        ("Shape".to_string(), Some(Ty::Struct("Circle".into()))),
        ("Shape".to_string(), Some(Ty::Struct("Square".into()))),
        ("Other".to_string(), Some(Ty::Struct("Guard".into()))),
    ];
    let decls = |name: &str| -> Option<DeclFacts> {
        match name {
            "Circle" | "Square" => Some(DeclFacts { has_user_drop: false, fields: vec![Ty::F64] }),
            "Guard" => Some(DeclFacts { has_user_drop: true, fields: vec![Ty::I32] }),
            _ => dyn_decls(name, &coerced),
        }
    };
    let shape = Ty::BoxDyn { trait_name: "Shape".into() };
    assert_eq!(resolve(&shape, &decls), ResourceKind::Memory);
    assert_eq!(resolve(&list(shape.clone()), &decls), ResourceKind::Memory);
    assert_eq!(resolve(&Ty::BoxDyn { trait_name: "Other".into() }, &decls), ResourceKind::Resource);
    assert_eq!(resolve(&Ty::BoxDyn { trait_name: "Unknown".into() }, &decls), ResourceKind::Memory);
}

/// One implementor with a destructor makes the trait object a resource, and a
/// coercion whose concrete type is not recoverable keeps the trait undecided.
#[test]
fn a_trait_object_with_a_resource_or_unknown_implementor_is_not_memory() {
    let coerced = vec![
        ("Shape".to_string(), Some(Ty::Struct("Circle".into()))),
        ("Shape".to_string(), Some(Ty::Struct("Guard".into()))),
    ];
    let decls = |name: &str| -> Option<DeclFacts> {
        match name {
            "Circle" => Some(DeclFacts { has_user_drop: false, fields: vec![Ty::F64] }),
            "Guard" => Some(DeclFacts { has_user_drop: true, fields: vec![Ty::I32] }),
            _ => dyn_decls(name, &coerced),
        }
    };
    assert_eq!(resolve(&Ty::BoxDyn { trait_name: "Shape".into() }, &decls), ResourceKind::Resource);

    let undecided = vec![("Shape".to_string(), None)];
    let decls = |name: &str| dyn_decls(name, &undecided);
    assert_eq!(resolve(&Ty::BoxDyn { trait_name: "Shape".into() }, &decls), ResourceKind::Declared);
}

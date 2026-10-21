//! KelpScript Tidepool heap, front end (WP2 of the collector design,
//! `kelpscript-gc-paper.md` sections 7 and 10).
//!
//! A hosted build keeps memory values in a collected heap and refuses resource
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
    /// A value a `fun` literal written directly as the `thread.spawn` entry
    /// captures (decision 127). Its environment is dropped by the worker's
    /// cleanup on every exit, so it is decided by the crossing rule, not by
    /// the closure rule.
    SpawnEntryCapture,
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
            HeapSlot::ClosureCapture | HeapSlot::SpawnEntryCapture => {
                "a value captured by a closure environment"
            }
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
    /// Whether a user destructor is allowed to cross. Only a thread argument
    /// lifts the rule (issue 233): the worker owns the value and its frame runs
    /// the destructor. A closure environment or a thread result keeps it out
    /// (decision 21).
    user_drop_crosses: bool,
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
        // language itself (`design/syntax.md (kelp wiki)` 11.5), not by this classifier.
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
    let mut walk = Walk { decls: &none, path: Vec::new(), user_drop_crosses: false };
    classify(ty, &mut walk)
}

/// The classification of a type given the program's declarations. A user
/// declaration with a destructor is a resource, and so is one with a resource
/// field. A declaration that `decls` does not know stays `Declared`.
pub fn resolve(ty: &Ty, decls: &dyn Fn(&str) -> Option<DeclFacts>) -> ResourceKind {
    let mut walk = Walk { decls, path: Vec::new(), user_drop_crosses: false };
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
        let kind = match u.slot {
            HeapSlot::ThreadArgument => thread_argument_kind(&u.ty, decls),
            HeapSlot::ThreadResult | HeapSlot::SpawnEntryCapture => crossing_kind(&u.ty, decls),
            _ => resolve(&u.ty, decls),
        };
        check_heap_slot(u.slot, &u.ty.display(), kind, u.span)?;
    }
    Ok(())
}

/// The channel records a thread may take as an argument or return (decision 125).
/// Their drop closes the channel, which is what a worker's panic must still run, so
/// the rule reads their fields and ignores their destructor.
pub const CROSSING_CHANNEL_RECORDS: &[&str] = &[
    "std.sync.Sender",
    "std.sync.Receiver",
    "std.sync.SendEnd",
    "std.sync.RecvEnd",
];

/// Whether a value of `ty` may cross into a worker as its argument or its result
/// (channels plan item 7). A `Mutex`, `RwLock` or `Condvar` crosses, and so does a
/// thread handle whose value crosses. A `Mutex` crosses only with a memory element,
/// because its element sits in a runtime record the collector does not scan. A user
/// declaration crosses when it has no destructor and every field crosses. Files,
/// sockets, a `Pool` (which has a destructor), destructors and every container with a
/// resource element stay refused.
fn crosses(ty: &Ty, walk: &mut Walk<'_>) -> bool {
    match ty {
        Ty::Mutex(elem) | Ty::RwLock(elem) => classify(elem, walk) == ResourceKind::Memory,
        Ty::Condvar => true,
        Ty::Thread(elem) => crosses(elem, walk),
        Ty::Struct(name) => struct_crosses(name, walk),
        _ => classify(ty, walk) == ResourceKind::Memory,
    }
}

fn struct_crosses(name: &str, walk: &mut Walk<'_>) -> bool {
    let base = name.split('<').next().unwrap_or(name);
    if CROSSING_CHANNEL_RECORDS.contains(&base) {
        // A generic instance is named `std.sync.Sender<i64>`, and its fields carry the
        // element type, so the fields decide it.
        let Some(facts) = (walk.decls)(name) else {
            return false;
        };
        return fields_cross(&facts.fields, name, walk);
    }
    if STDLIB_RESOURCE_STRUCTS.contains(&name) {
        return false;
    }
    if STDLIB_MEMORY_STRUCTS.contains(&name) {
        return true;
    }
    let Some(facts) = (walk.decls)(name) else {
        return false;
    };
    // A user destructor keeps a value from crossing, except as a thread argument:
    // there the worker owns it and its frame runs the destructor on every exit,
    // panic included (issue 233). Its fields still have to cross.
    if facts.has_user_drop && !walk.user_drop_crosses {
        return false;
    }
    fields_cross(&facts.fields, name, walk)
}

fn fields_cross(fields: &[Ty], name: &str, walk: &mut Walk<'_>) -> bool {
    // A type already on the path is the recursion, which adds nothing new.
    if walk.path.iter().any(|p| p == name) {
        return true;
    }
    walk.path.push(name.to_string());
    let ok = fields.iter().all(|field| crosses(field, walk));
    walk.path.pop();
    ok
}

/// The verdict for a value a thread takes as its argument or returns through `join`:
/// `Memory` when it crosses, otherwise the heap verdict, so the refusal names the
/// reason. Heap slots other than the two thread slots keep `resolve`.
pub fn crossing_kind(ty: &Ty, decls: &dyn Fn(&str) -> Option<DeclFacts>) -> ResourceKind {
    crossing_kind_with(ty, decls, false)
}

/// The crossing verdict for a thread argument: as [`crossing_kind`], except that
/// a user destructor whose fields cross is let in (issue 233).
pub fn thread_argument_kind(ty: &Ty, decls: &dyn Fn(&str) -> Option<DeclFacts>) -> ResourceKind {
    crossing_kind_with(ty, decls, true)
}

fn crossing_kind_with(ty: &Ty, decls: &dyn Fn(&str) -> Option<DeclFacts>, user_drop_crosses: bool) -> ResourceKind {
    let mut walk = Walk { decls, path: Vec::new(), user_drop_crosses };
    // A closure value itself never crosses as an argument or result: its environment
    // is a heap object the start record does not root (decision 131). A closure held
    // inside a crossing declaration, such as the pool's `Job`, is unchanged.
    if !matches!(ty, Ty::Closure { .. }) && crosses(ty, &mut walk) {
        return ResourceKind::Memory;
    }
    match resolve(ty, decls) {
        ResourceKind::Memory => ResourceKind::Resource,
        kind => kind,
    }
}
#[cfg(test)]
#[path = "managed/tests.rs"]
mod tests;

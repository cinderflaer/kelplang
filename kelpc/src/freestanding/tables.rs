/// One shipped module's place in a freestanding program (kernel stage 19,
/// `kelp wiki page language/freestanding.md`). The row for each module is a decision derived
/// by building that module into a riscv64 image, not from its prose: a module
/// whose bodies all resolve is admitted, one that leaves an undefined symbol is
/// either narrowed to the functions that do not or refused whole.
pub enum StdAdmission {
    /// Every item is callable, and no body reaches libc or the allocator.
    Pure,
    /// Every item is callable, but the bodies need the manifest's `[target]
    /// alloc`/`dealloc` pair; without it the module is refused with the two keys
    /// named.
    Allocator,
    /// Only the named items are callable. `callable` is the set that needs
    /// nothing, `with_allocator` the set that needs the allocator pair, and
    /// `helpers` the private bodies a kept item reaches. Every other item is
    /// refused by name at the call, and `prune` drops its body.
    Subset {
        callable: &'static [&'static str],
        with_allocator: &'static [&'static str],
        helpers: &'static [&'static str],
    },
    /// The module may be part of the program because another module imports
    /// it, but no item is callable.
    Present,
    /// Nothing of the module may be part of a freestanding program.
    Hosted,
}

/// The `std/str` functions that need no allocator: a `str` is a NUL-terminated
/// literal in read-only data, so the compiler lowers these as byte loops inside
/// the module (`codegen_llvm::emit_freestanding_str_bodies`), and
/// `starts_with_at` is the one private body `eq`/`starts_with` reach.
pub(crate) const STR_CALLABLE: &[&str] = &["len", "byte", "eq", "starts_with", "ptr"];
pub(crate) const STR_HELPERS: &[&str] = &["starts_with_at"];

/// The `std/str` bodies that build an owned `String`, so they are callable only
/// with the allocator: without one they would reference `kelp_String__*`
/// symbols the project cannot define.
pub(crate) const STR_WITH_ALLOCATOR: &[&str] = &["concat", "from_u64", "from_i64"];

/// `std/option`'s and `std/result`'s value helpers, which are `when`/`if let`
/// and a `panic`, and their callback combinators, whose environments are heap
/// boxes: the callback set is what a freestanding image leaves `free`
/// undefined for without an allocator.
pub(crate) const OPTION_CALLABLE: &[&str] = &["is_some", "is_none", "unwrap_or", "unwrap", "expect"];
pub(crate) const OPTION_WITH_ALLOCATOR: &[&str] = &["map", "and_then", "or_else", "each"];
pub(crate) const RESULT_CALLABLE: &[&str] = &[
    "is_ok",
    "is_err",
    "unwrap_or",
    "unwrap",
    "unwrap_err",
    "expect",
    "expect_err",
];
pub(crate) const RESULT_WITH_ALLOCATOR: &[&str] = &["map", "map_err", "and_then", "or_else", "each"];

/// `std/math`'s integer functions: one-line Kelp comparisons, against the
/// module's sixteen libm externs and its `f64` overloads, which are the rest of
/// it. Names are matched through the overload suffix (`std.math.min@i32,i32`),
/// and every overload that mentions `f32` or `f64` is refused by name in
/// `item_is_callable`: on riscv64 a float comparison is a soft-float libcall
/// (`__ltdf2`, `__ltsf2`), which no bare-metal image has.
pub(crate) const MATH_CALLABLE: &[&str] = &["abs", "min", "max", "clamp"];

/// The table. Every id `stdlib::shipped()` returns has a row here (a unit test
/// below pins that), and an id that does not is `Hosted`: the safe default for
/// a module this file has not decided about.
pub fn std_admission(module: &str) -> StdAdmission {
    match module {
        // `std/arena` is pure pointer arithmetic and needs no allocator; `std/irq`
        // and `std/frame` are a `datac` and two methods each, so they carry
        // their capability by construction and their numbers come from the
        // caller's grant, not from a runtime.
        "std/atomic" | "std/arena" | "std/irq" | "std/frame" | "std/convert" | "std/ops" => {
            StdAdmission::Pure
        }
        // `std/list` exists only to carry function values over a `List<T>`: all
        // six bodies take a closure, whose environment and result list are the
        // allocator's. `std/regex` is a pure-Kelp engine whose one allocation
        // is the compiled program and its scratch (`List`/`String`), so it is
        // the same shape: every item is callable once the pair is configured.
        "std/list" | "std/regex" => StdAdmission::Allocator,
        // `std/iter` is a `datac` and a handful of `while` loops, so `range`,
        // `range_step`, `count` and `sum_i64` build bare-metal. `collect_list`
        // allocates its result list, so it waits for an allocator. A method is
        // named here by its bare name, because `prune` reads `FunctionDecl::name`
        // and a receiver method's name carries no receiver: the one method is
        // `Range.next`, and a `for` over an `iter.range` lowers to a call to it.
        crate::stdlib::ITER_MODULE => StdAdmission::Subset {
            // A receiver method is named by its bare name: `prune` reads
            // `FunctionDecl::name`, and `item_is_callable` strips the `Type#`
            // from a call key, so both sides agree on `next`.
            callable: &["next", "range", "range_step", "count", "sum_i64"],
            with_allocator: &["collect_list"],
            helpers: &[],
        },
        STD_STR => StdAdmission::Subset {
            callable: STR_CALLABLE,
            with_allocator: STR_WITH_ALLOCATOR,
            helpers: STR_HELPERS,
        },
        "std/option" => StdAdmission::Subset {
            callable: OPTION_CALLABLE,
            with_allocator: OPTION_WITH_ALLOCATOR,
            helpers: &[],
        },
        "std/result" => StdAdmission::Subset {
            callable: RESULT_CALLABLE,
            with_allocator: RESULT_WITH_ALLOCATOR,
            helpers: &[],
        },
        "std/math" => StdAdmission::Subset {
            callable: MATH_CALLABLE,
            with_allocator: &[],
            helpers: &[],
        },
        // `std/mem`'s one extern is libc `malloc`, and `std/rt` is injected for a
        // capability domain; both may be present and neither is callable.
        "std/mem" | crate::stdlib::RT_MODULE => StdAdmission::Present,
        // `std/tz` reads TZif files and `$TZDIR`: a bare-metal image has no
        // filesystem and no environment for it to read, and its `Result`
        // payloads carry `String` (kelp wiki page stdlib/tz.md, Freestanding).
        "std/tz" => StdAdmission::Hosted,
        _ => StdAdmission::Hosted,
    }
}

/// Whether a call into an admitted module may be written. `allocator` is
/// whether the manifest configured `[target] alloc`/`dealloc`. One predicate so
/// the call-site check and `prune` cannot disagree about which bodies exist.
pub fn item_is_callable(module: &str, item: &str, allocator: bool) -> bool {
    match std_admission(module) {
        StdAdmission::Pure => true,
        StdAdmission::Allocator => allocator,
        StdAdmission::Subset {
            callable,
            with_allocator,
            helpers,
        } => {
            // An overloaded name is declared once per numeric type, and a call
            // names the one it picked (`std.math.min@i32,i32`), so the table's
            // bare name is what matches. Every float overload is refused
            // whatever the table says: a `f32` or `f64` comparison on the
            // kernel's riscv64 triple is a soft-float libcall, not an
            // instruction. A
            // receiver method's call key carries its type (`Range#next`),
            // while the table names the method (`next`), so the receiver is
            // stripped too.
            let item = item.rsplit('#').next().unwrap_or(item);
            let base = item.split('@').next().unwrap_or(item);
            let allowed = callable.contains(&base) || helpers.contains(&base);
            let allocator_only = allocator && with_allocator.contains(&base);
            (allowed || allocator_only) && !(module == "std/math" && is_float_overload(item))
        }
        StdAdmission::Present | StdAdmission::Hosted => false,
    }
}

/// Whether a parameter or type name is a hardware-float type, whose comparisons
/// are soft-float libcalls on a target without the F/D extensions.
pub(crate) fn is_float_type(ty: &str) -> bool {
    ty == "f32" || ty == "f64"
}

/// Whether an overload key (`min@f32,f32`) names a float overload.
pub(crate) fn is_float_overload(item: &str) -> bool {
    item.contains("f32") || item.contains("f64")
}

/// Whether `item` of `module` would be callable if an allocator were
/// configured: what the refusal for a function that only needs the pair says.
pub(crate) fn item_needs_allocator(module: &str, item: &str) -> bool {
    let base = item.split('@').next().unwrap_or(item);
    match std_admission(module) {
        StdAdmission::Allocator => true,
        StdAdmission::Subset { with_allocator, .. } => {
            with_allocator.contains(&item) || with_allocator.contains(&base)
        }
        _ => false,
    }
}

/// The modules a freestanding project may implement, sorted, derived from the
/// table: what a refusal names so the reader is told what would work instead.
pub fn allowed_modules() -> String {
    let mut ids: Vec<&str> = crate::stdlib::shipped()
        .iter()
        .copied()
        .filter(|id| !matches!(std_admission(id), StdAdmission::Present | StdAdmission::Hosted))
        .collect();
    ids.sort_unstable();
    ids.join(", ")
}

/// The one module that is partly callable, named here because the rest of the
/// compiler refers to it by id.
pub const STD_STR: &str = "std/str";

/// The `String` methods a freestanding build lowers itself, through the project
/// allocator (kernel stage 12, MK9-S5-M2). Every other method on the type needs
/// the hosted runtime, because its body is a `strlen`/`memcpy`/`realloc` call
/// or a formatting primitive; the call-site check refuses each by name.
/// `with_capacity`, `reserve` and `clear` are the buffer primitives `concat`
/// and the integer builders are written on top of, so they are here too.
pub const ALLOWED_STRING_METHODS: &[&str] = &[
    "empty",
    "from_str",
    "len",
    "is_empty",
    "as_str",
    "view",
    "clone",
    "push",
    "push_str",
    "reserve",
    "with_capacity",
    "clear",
];

/// The `View` methods a freestanding build lowers itself (kernel stage 23).
/// `len`/`is_empty` read the length word, `get`/`slice`/`eq` are byte loops and
/// pointer arithmetic with a bounds panic, and `to_string` is the one that
/// allocates, so it lives in `ALLOWED_VIEW_WITH_ALLOCATOR` like every other
/// `String` builder.
pub const ALLOWED_VIEW_METHODS: &[&str] = &["len", "is_empty", "get", "slice", "eq"];

/// The one `View` method whose body allocates a `String`, and therefore needs
/// the manifest's `[target] alloc`/`dealloc` pair.
pub const ALLOWED_VIEW_WITH_ALLOCATOR: &[&str] = &["to_string"];

/// The `Bytes` methods a freestanding build lowers itself. Same rule as the
/// `String` list: anything that has no libc-free body (the endian accessors are
/// compiled, but `as_str`'s UTF-8 validation and `append`'s growth are not on
/// this stage's list) is refused by name at the call.
pub const ALLOWED_BYTES_METHODS: &[&str] = &[
    "empty",
    "from_str",
    "len",
    "is_empty",
    "get",
    "set",
    "push",
    "clone",
    "reserve",
    "clear",
];

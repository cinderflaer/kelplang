# The freestanding standard subset (kernel stage 19, release goal 3)

*A design note, not a Mark. `plan/kernel-path.md` owns the stage list;
`plan/freestanding.md` owns the mode. This file is the record of the decision
this stage made: which parts of `std` work in a freestanding project, and how
the compiler admits them.*

## What this stage changes

Before it, `freestanding.rs` kept two lists: the modules whose items are all
callable (`std/atomic`, `std/option`, `std/result`, `std/arena`, `std/irq`,
`std/frame`) and the modules that may be *present* without being callable
(`std/str`, `std/mem`, `std/rt`). Every other shipped module was refused
wholesale by the module loop, and `std/math` was refused with them.

This stage replaces both lists with one table, `freestanding::std_admission`,
whose row for each shipped module is one of:

- `Pure`: every item is callable and no body reaches libc or the allocator.
- `Allocator`: every item is callable once the manifest configures
  `[target] alloc`/`dealloc`; without the pair the module is refused with the
  two keys named.
- `Subset`: only the named items are callable. `callable` is the set that needs
  nothing, `with_allocator` the set that is callable only with the allocator
  pair, and `helpers` the private bodies a kept item reaches. Every other item
  is refused by name at the call, and `prune` drops its body so a dead libc
  reference is never emitted.
- `Present`: the module may be part of the program (another module imports it)
  but no item is callable.
- `Hosted`: nothing of the module may be part of a freestanding program, and
  the program is refused by name with the allowed-module list.

The table is checked against `stdlib::shipped()` by a unit test, so adding a
shipped module without a verdict fails the build rather than defaulting.

## The table

Every module is `stdlib::shipped()`'s list plus `std/rt` (the resolver-injected
matcher, not a library a program imports) and `std/bits`, which this branch
does not ship.

| Module | Verdict | Evidence |
| --- | --- | --- |
| `std/atomic` | works as-is | `Atomic<u32>.new`/`load`/`store`/`compare_exchange` build into a riscv64 image with no undefined symbol. `Ordering` is a plain `enum`. |
| `std/option` | works as-is for `is_some`, `is_none`, `unwrap_or`, `unwrap`, `expect`; `map`, `and_then`, `or_else`, `each` work with an allocator | `option.is_some`/`unwrap` link. A callback call links with an undefined `free` (through `kelp_coerce_drop__Closure__...`) without an allocator, and links clean with one, for a declared `fun` and for a capturing literal alike. |
| `std/result` | the same split as `std/option`: value helpers as-is, and `map`, `map_err`, `and_then`, `or_else`, `each` with an allocator | `result.is_ok`/`unwrap` link; `result.map` leaves `free` undefined without an allocator. |
| `std/bits` | not shipped on this branch | No `kelpc/std/bits.kp` and no `stdlib::shipped()` entry, so the stage that uses it names `option`/`result`/`arena` instead. |
| `std/arena` | works as-is | `arena.new`/`alloc`/`reset` build and boot with no allocator configured; the region is the caller's. |
| `std/irq` | works as-is | `irq.claim`/`Line.number` build under an `irq` grant and boot in `examples/kernel0-caps`. |
| `std/frame` | works as-is | `frame.claim`/`Frame.addr`/`offset` build under a `frames` grant and boot in `examples/kernel0-caps`. |
| `std/str` | works as-is for `len`, `byte`, `eq`, `starts_with`, `ptr`; `concat`, `from_u64`, `from_i64` work with an allocator; every other function is refused by name | The five lower to byte loops in the object (`emit_freestanding_str_bodies`). `concat` and the integer builders need an owned `String`, so their bodies are kept only with an allocator. `from_f64` is `snprintf` and is refused by name with "needs the hosted runtime". |
| `std/math` | works after the small change this stage makes: the integer overloads of `abs`, `min`, `max` and `clamp` are admitted by name; the sixteen extern-backed functions (`sqrt`, `pow`, `floor`, `ceil`, `round`, `trunc`, `exp`, `ln`, `log2`, `log10`, `sin`, `cos`, `tan`, `atan2`, `is_nan`, `is_finite`), the `f64` overloads of the four arithmetic ones and `sign` are refused by name | Today the whole module is refused. Admitting the module without a per-item set is not enough: every concrete body is emitted, so an uncalled `sqrt` left `undefined symbol: sqrt` at `ld.lld`, and an admitted `sign` left `__ltdf2`, the soft-float compare a riscv64 image has no lib for. The integer overloads are one-line Kelp comparisons, so the subset is separable by name and by parameter type (a call key is `std.math.min@i32,i32`, and `prune` drops an overload whose parameters mention `f64`). |
| `std/list` | works with an allocator | With the module admitted and the allocator pair configured, `list.map`, `list.any` and a capturing closure all link with zero undefined symbols; the callback's environment is allocated and freed through the project's functions. Without the pair, `List<T>` is refused exactly as `Box`/`Map` are. |
| `std/mem` | refused: its one extern, `alloc_raw`, is `kelp_bytes_alloc_raw` (libc `malloc`/`memset`) | With the module admitted the link fails `undefined symbol: kelp_std__mem__alloc_raw`; the module stays present only because `std/str` imports it, and `prune` drops its body. |
| `std/fmt` | refused: `hex_raw`/`bytes_raw` are `snprintf`-backed and `float` reaches `str.from_f64` | With the module admitted the build fails inside `std/fmt` on the `str.from_f64` that pruning removed, so the refusal is the clear answer rather than a second-order error. |
| `std/encoding` | refused: it imports `std/fmt`, and its encoders need `String`/`Bytes` | The module loop refuses it by name. |
| `std/path` | refused: normalization goes through the runtime's `path_normalize` and `str` functions outside the subset | Refused by name. |
| `std/io` | refused: it carries file-descriptor passdowns and `read`/`write` | Refused by name (the module loop now runs before the passdown check, so the message names `std/io`). |
| `std/fs` | refused: libc `fopen`/`scandir`/`rename` | Refused by name. |
| `std/net` | refused: libc sockets, DNS and `poll` | Refused by name. |
| `std/tls` | refused: OpenSSL and libc | Refused by name. |
| `std/http` | refused: it needs `std/net`/`std/tls` | Refused by name (or by the capability delegation check that names it). |
| `std/ws` | refused: it needs `std/net`/`std/tls`/`std/encoding` | Refused by name. |
| `std/process` | refused: libc `fork`/`execve`/`waitpid` | Refused by name. |
| `std/env` | refused: libc `getenv`/`setenv` | Refused by name. |
| `std/time` | refused: `clock_gettime`/`nanosleep` | Refused by name. |
| `std/rand` | refused: the OS entropy source | Refused by name. |
| `std/thread` | refused: pthreads and the hosted runtime | Refused by name. |
| `std/sync` | refused: `Mutex<T>`'s runtime half is a pthread mutex | Refused by name, separately from the `Mutex` type refusal. |
| `std/rt` | present, not callable | The resolver injects it for a module that holds a capability domain; the `irq`/`frames` checks are front-end and emit no matcher call, and `prune` drops the module's bodies. |

## The allowed list a refusal names

Every refusal of a whole module says "needs a hosted runtime" and then names
the modules a freestanding project may implement, derived from the table (so it
cannot drift): `std/arena, std/atomic, std/convert, std/frame, std/irq,
std/list, std/math, std/option, std/result, std/str`. `std/list` needs the
allocator pair, and several functions inside the `Subset` rows are refused by
name; both are said in the message of the construct that needs them, not in the
list. `std/convert` is pure language (two traits and no runtime), so a kernel
may use `From` exactly as a hosted program does.

## Tests

- One riscv64 build project per admitted module
  (`tests/projects/freestanding_std_*`), each importing the module and calling a
  representative function, built and checked with `llvm-nm -u` for zero
  undefined symbols.
- `freestanding_std_alloc` is the allocator half: `std/list`, the
  `std/option`/`std/result` callback combinators and `str.concat`/`from_u64`.
- `freestanding_std_kernel` boots under QEMU and prints `std/option`,
  `std/result` and `std/arena` results over the real UART.
- Rejection fixtures: `freestanding_std_refused_time`,
  `freestanding_std_refused_thread`, `freestanding_std_refused_env` (three
  whole modules) and `freestanding_std_math_refused` plus
  `freestanding_std_option_no_alloc` (one refused function inside an admitted
  module, one needing libc and one needing the allocator).

## What this stage does not do

- No new std function: what is admitted is what the shipped modules already
  declare. `str.is_empty` and the rest of the `std/str` surface stay out, and
  `std/math`'s libm functions stay out even though a kernel could own a
  software implementation of them.
- No freestanding `std/mem`. Rerouting `alloc_raw` through `[target] alloc` is
  a change to a body, not a decision about a module, and nothing has asked for
  it.
- No `std/bits`: the module does not exist on this branch.

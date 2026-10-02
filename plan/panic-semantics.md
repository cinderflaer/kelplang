# Panic semantics with no OS (MK9-S6-M2, kernel stage 15)

*A design note, not a Mark. `plan/kernel-path.md` owns the stage list; this is
the record of what this stage decided and why. `plan/freestanding.md` owns the
freestanding mode this is part of.*

## What a panic is on bare metal

There is no process to kill and no stderr to print to, so a check that fires
has to say what it caught and where, and then stop the machine. This stage
gives the compiler's own checks a real message and a source location and gives
a panic that fires *while the handler runs* one final place to go, instead of
recursing into the same check forever.

## The handler's two shapes

`[target] panic` names a `fun` in the entry module. Two shapes are accepted:

- the old one-argument form, `fun kp_panic(msg: str)`, unchanged, and
- `fun kp_panic(msg: str, file: str, line: u32)`, which also receives the
  source *file name* (`main.kp`, the last component of the module's path, the
  same spelling a diagnostic's `-->` ends with) and the 1-based *line* of the
  statement the check was inserted into.

Any other shape is a compile error that names both accepted forms. The
location is the same per-statement span the hosted backends' line tables and
`#line` directives are built from (`diag::SourceMap::path_line_col`), so a
freestanding panic names the same line a hosted one would.

The `line` is the statement the check belongs to, not the exact sub-expression:
one `locate` per statement is what both backends already carry, so this is the
same granularity the hosted side has.

## The emitted entry and the double panic

A check never calls the project's handler directly. It calls one
compiler-emitted entry, `kelp_panic_entry(msg, file, line)`, which:

1. atomically exchanges a flag (`kelp_panic_active`, an `i32` in `.bss`) from
   0 to 1,
2. if the old value was 0 this is the first panic, so it calls the project's
   handler (with one or three arguments, by the handler's own shape), and
3. if the old value was 1 a panic is already in progress, so it calls
   `kelp_double_panic()` and halts if that returns.

`kelp_double_panic` is emitted as a **weak** definition whose body loops on the
target's wait-for-interrupt instruction (`wfi` on riscv64, `wfe` on aarch64),
exactly as the `mem*` fallbacks are weak (`plan/freestanding.md`). A project
that supplies its own `kelp_double_panic` (a strong definition in an object
linked beside the image, the same way `tests/projects/freestanding_memcpy_user`
supplies its own `memcpy`) wins at link time and can print instead of wedging.
The weak halt is what "else halts" means: it is always defined, so the image
has no undefined symbol, and only a project definition ever overrides it.

The flag is an atomic exchange rather than a load/store so two harts that
panic at once cannot both think they are first.

## Where a check's location comes from

The compiler keeps two module globals, `kelp_panic_file` (a `ptr` to the
current file-name string) and `kelp_panic_line` (an `i32`). Before every
statement of a `.kp` function the emitter stores that statement's file and
line into them; a check emitted later in that statement loads them and passes
them to `kelp_panic_entry`.

This is what reaches the checks that live in *generated* bodies (`Array.get`'s
bounds check, `List.get`, `Bytes.get`, the allocator's out-of-memory path):
those bodies are emitted once per type and are shared by every call site, so
they cannot carry a per-call-site constant. The caller's statement is the last
thing stored, so a panic inside the generated body names the user's line. A
panic raised from a check the compiler emits *inline* in the user's own
function (`to_u8`'s range check, `panic(msg)`, `cpu_index()`) reads the same
two globals.

The cost is two stores per `.kp` statement in a freestanding build. That is a
debug-time diagnostic, not a fast path, and a hosted build pays nothing: the
globals are declared and written only when `[target] mode = "freestanding"`.

## The checks and their messages

Every compiler-inserted check in a freestanding build calls
`kelp_panic_entry` with the message below and the location above.

| Check | Message | Where emitted |
| --- | --- | --- |
| `Array.get`/`get_const`/`set` index | `Array.get: index out of bounds` | generated body |
| `List.get`/`get_const`/`set` index | `List.get: index out of bounds` | generated body |
| `List.pop` on an empty list | `List.pop: the list is empty` | generated body |
| `List.insert` index | `List.insert: index out of bounds` | generated body |
| `List.remove` index | `List.remove: index out of bounds` | generated body |
| `List.slice` range | `List.slice: range out of bounds` | generated body |
| `Box.get` on an empty box | `Box.get: the box is empty` | generated body |
| `Box.take` on an empty box | `Box.take: the box is empty` | generated body |
| `Map.get` missing key | `Map.get: key not found` | generated body |
| `Map.remove` missing key | `Map.remove: key not found` | generated body |
| `Bytes.get`/`set` index | `Bytes.get: index out of bounds` | generated body |
| `Bytes.read_*`/`write_*` offset | `Bytes.read_u32: offset out of bounds` (per accessor) | inline |
| `View.get` index | `View.get: index out of bounds` | generated body |
| `str.byte` index | `str.byte: index out of bounds` | generated body |
| narrow conversion | `i32.to_u8: value out of range` (per call) | inline |
| allocator failure | `out of memory` | generated body |
| `panic(msg)` | the program's own message | inline |
| `std/option.unwrap`/`expect` | `Option.unwrap: value is None` (its own `panic`) | inline |
| `std/arena.alloc` bad alignment | its own `panic` | inline |
| `cpu_index()` out of range (debug) | `cpu index out of range` | inline |

Two things the task lists as checks are **not** checks in this language, and
the table records that rather than inventing one:

- **integer overflow** wraps. There is no `nsw`/`nuw` on any arithmetic
  instruction and no runtime overflow check (`plan/kernel-ub-notes.md`,
  `plan/kernel0-status.md` gap 3).
- **division by zero** is not checked. `sdiv`/`udiv` by zero is UB in LLVM
  exactly as it is in C; the only guard is constant folding in the front end.

## No handler configured

With no `[target] panic`, the compiler still emits `kelp_panic_entry` and the
weak `kelp_double_panic`, and the entry's first-panic branch calls the
generated infinite-loop handler. A check still stops the machine; it just
prints nothing, which is the pre-stage behaviour.

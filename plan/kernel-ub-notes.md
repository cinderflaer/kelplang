# The undefined-behaviour catalogue, stage by stage

One section per kernel-path stage: what new way to trigger undefined or
implementation-defined behaviour the stage adds, and what the compiler does and
does not check. `plan/kernel-path.md` is the stage list; `plan/unsafe-and-pointers.md`
is stage 1's design.

## Stage 1: MK9-S2 `unsafe` and pointers

**New ways to reach UB.** `Ptr<T>` is a bare machine address with no provenance,
no lifetime and no ownership, and it may be null. Every unsafe operation is a
promise the compiler does not check:

- `p.read()` / `p.write(v)` on an address that is not a live, correctly aligned
  `T`, or on memory of a different type, is UB. `Ptr.from_addr(a)` accepts any
  integer, so a program can name any address, including one it was never
  granted.
- `p.offset(n)` / `p.byte_offset(n)` scale or step with no bounds check and wrap
  around the address space; a resulting address that does not point inside one
  allocated object is UB to dereference.
- `p.read_volatile()` / `p.write_volatile(v)` make the *access* unmergeable,
  unreorderable and unelidable, but they say nothing about the address being
  valid, and they are not a synchronisation mechanism.
- `Ptr.alloc<T>(count)` returns uninitialised memory, and reading a slot before
  writing it reads an indeterminate value. `p.free()` on an address not from
  `alloc`, twice, or while another pointer is still used, is UB.
- `Ptr.from_ref(&x)` / `Ptr.from_mut(x)` produce a pointer that does **not**
  extend the borrow: using the pointer after the place is gone (a returned
  pointer to a dead frame, a pointer kept past a `move`) is UB, and writing
  through a pointer derived from a *shared* borrow is a data race with any
  reader.
- Mixing a `Ptr<T>` with `Atomic<T>` over the same bytes is a data race; the
  language has no rule that ties the two operations to one location.

**What the compiler does check.** That an `unsafe` operation is written inside
`unsafe { ... }` or an `unsafe fun`; that a module containing `unsafe` is listed
in `coral.toml`'s `[unsafe] modules`; that a `Ptr<T>` element is a type with a
machine-level layout; that `read`/`read_volatile` only read `Copy` elements (so
no second owner of an owned value is made); and that the volatile accesses are
emitted as volatile access on both backends.

**What it does not check.** Anything about the address, its alignment, its
provenance, its lifetime, or whether it is still allocated. There is no
provenance model and no aliasing model yet (roadmap D-1); this stage is the
first thing those have to describe.

## Stage 2: MK9-S1-M2 layout

**New ways to reach UB.** Layout control makes the address arithmetic stage 1
already allowed easier to get wrong, and adds one promise about an address:

- `repr(packed)` puts a field at an offset its own type is not aligned for.
  The language has no pointer-to-field that escapes, so reading the field
  through the value is fine, but `offset_of<Packed>("field")` plus `Ptr`
  arithmetic can build a misaligned `Ptr<T>`; `read`/`write` there is UB for
  the same reason it always was, and the packed clause is what makes it easy.
- `repr(align(N))` is a **promise** that a value of the type is `N`-aligned.
  The compiler aligns every alloca and global it creates (the C attribute and
  the LLVM alignment anchor are the same fact), but a value reached through a
  `Ptr` it did not create (`Ptr.from_addr`, a cast, a `byte_offset`) carries no
  such guarantee, and reading or writing it as the aligned type is UB.
- A `repr(packed)` type read through a `Ptr` to a *different* type is the same
  type-punning UB as before, now more tempting because the packed bytes are easy
  to reinterpret.
- A read of an address built from `offset_of` and `Ptr` arithmetic is UB when
the offset does not land inside the same allocated object, exactly the stage 1
rule; `offset_of` only reports the offset, it does not make the address valid.

**What the compiler does check.** That a `repr` clause is one of `c`, `packed`,
`align(N)` or an integer type in the right position; that `N` is a power of two;
that an explicit discriminant is written only on a field-less enum, is unique,
and fits the tag's storage type; that a layout query names a type with a
machine-level layout and, for `offset_of`, a field the type has. `size_of`,
`align_of` and `offset_of` are compile-time constants, so they add no runtime
behaviour at all; they only let a program compute an address the compiler still
does not validate.

**What it does not check.** The alignment of any address a program builds, or
that memory reached through a `Ptr` holds a value of the type it is read as.
There is still no provenance model (roadmap D-1); `repr` makes the alignment
part of D-1's description rather than adding a check.

## Stage 3: MK9-S8 data formats

**New ways to reach UB.** This stage adds one new promise and one new
unchecked offset, both in unsafe code or on a caller-controlled one:

- `Ptr.read_unaligned`/`write_unaligned` drop the *alignment* half of what
  `read`/`write` already required, and nothing else. The address still has to
  point at a live, initialised `T` inside one allocated object, exactly the
  stage 1 rule; an unaligned access to a valid object is defined, and an
  aligned access to a bad address is still UB. There is no provenance model
  (roadmap D-1), so the compiler cannot tell either case from the other.
- The `Bytes` byte-order readers and writers take a caller-supplied `usize`
  offset. The compiler checks nothing about where a `Bytes` came from, but the
  runtime bounds check (`offset + width <= len`, with the wrapped addition a
  failure too) means an offset outside the buffer panics instead of reaching
  the memory, so the offset alone cannot make these two UB.

**What the compiler does check.** That the byte-order methods are written on
an integer or a `Bytes` (the associated `u32.from_be(x)` forms take a value of
the same type); that a `Bytes` write has a `mut` receiver, so the borrow
checker sees the exclusive borrow the mutation needs; that
`Ptr.read_unaligned` only reads a `Copy` element; and that both unaligned
operations are inside an `unsafe` block or an `unsafe fun` in a module
`coral.toml` lists under `[unsafe] modules`. Both backends emit the unaligned
access as an align-1 load or store (`align 1` on LLVM, `memcpy` on C) and the
swap as `llvm.bswap.*` / `__builtin_bswap*`, and `to_le`/`to_be` read the
target's own byte order rather than the host's.

**What it does not check.** The alignment of any address, as before, and
nothing about a `Bytes` beyond its length: whether the bytes at an offset hold
the field the program says they do is the program's claim about the format,
not the type system's. The two APIs are also not bounds-checked against each
other: a `write_u32_le` may overwrite a field a `read_u16_le` expects, and the
language has no rule that ties two accesses to one location (roadmap D-1).

## Stage 4: MK9-S4-M1/M2 freestanding

**New ways to reach UB.** This stage removes the ambient runtime and hands the
program machine state, so the ways to be wrong are the ones C has:

- `_start` sets `sp` from the linker-script symbol `__stack_top`. If the script
  defines it over program data, over the vector table or below the image, the
  first push corrupts something; the compiler checks that a script was named,
  not what it contains. The stub clears nothing else, so it runs with whatever
  the boot loader left in every other register and CSR.
- The entry function's body is ordinary Kelp code, but nothing has run yet: a
  `String` or a `List` there would call an allocator that does not exist, which
  is why those types are refused, not why they are safe. `Atomic<T>` on bare
  metal is only as correct as the target's memory model makes it, and the
  language still publishes no model for a `Ptr<T>` and an `Atomic<T>` over the
  same bytes (stage 1's rule).
- A `section("...")` clause names an object-file section a script may or may
  not place. A clause naming a section the script does not mention silently
  drops the code or data from the image; the compiler only checks the clause's
  syntax and, for a `const`, that the value is a fixed-size integer array.
- The user's panic handler is a promise the compiler does not check. Nothing
  requires it to diverge, so a handler that returns comes back to the bounds or
  overflow check that called it, which then falls through into whatever the
  check was guarding. A user-written `panic(msg)` is worse: the backend lowers
  it to a call to the handler followed by LLVM's `unreachable`, so a handler
  that returns is undefined behaviour there, not a fallthrough. The default
  handler (an infinite loop) is the only one the compiler itself guarantees
  does not return.

**What the compiler does check.** That a `[target]` section names a `mode` and,
for freestanding, a `triple`, an `entry` and a `linker_script`; that the entry
is a `fun` in the entry module with no parameters and no return; that a named
panic handler is a `fun(msg: str)` with no return; and the allowed set of
`src/freestanding.rs`, construct by construct, on both `kelpc check` and
`kelpc build`. It also refuses the C backend for a freestanding project rather
than producing an image with libc in it.

**What it does not check.** Anything about the linker script's contents, the
address `__stack_top` names, the alignment or reachability of a section, or
whether the panic handler diverges. The generated `_start` and the script are
one contract split across two files, and only a boot (the next stages) tests it
end to end.

## Stage 5: MK9-S7-M1/M3 inline asm and naked functions

**New ways to reach UB.** This stage hands the assembler to the program, and
the compiler stops seeing inside it:

- `asm(...)` executes text the compiler does not read. Every promise about
  what the instruction does is the author's: a template that writes a register
  no `clobber("...")` names, that reads memory under `nomem`, or that jumps
  somewhere leaves the compiler's own register and memory assumptions wrong,
  and the wrongness is silent at build time. `nomem` is exactly a promise that
  the asm touches no memory; a false one lets LLVM reorder loads and stores
  across the instruction.
- The constraint names are passed through. `"r"` and `"rax"` are never checked
  against the operand's type, so a `f64` operand under an integer-register
  constraint, or a register the target does not have, reach the assembler and
  fail there (GCC) or produce code LLVM then emits (LLVM). The compiler's
  refusal set is the operand *shape*, not the ABI.
- `out("...") place` and `inout("...") place` write a place without the
  compiler knowing what value arrives. `inout` reads the place first, so the
  value the asm sees is whatever was there, which for an uninitialised `var` is
  indeterminate.
- A `naked fun` has no prologue, no epilogue and no compiler-generated `ret`.
  A body whose asm does not return falls through into whatever the bytes after
  it are, and the compiler emits nothing to prevent that. The declared return
  type is a caller-side promise only: the caller reads the return register
  (System V `rax`, riscv `a0`) whatever the asm left there.
- `nostack` is accepted and changes nothing: neither LLVM's inline-asm IR nor
  GCC's extended asm has a stack-clobber spelling, so the compiler cannot act
  on the promise even when it is true.
- The dialect is chosen by target: Intel syntax on x86, the target's own
  syntax elsewhere. A template written for one target's syntax assembled for
  another is an assembler error, not a language error.

**What the compiler does check.** That `asm` is inside `unsafe { ... }` or an
`unsafe fun` and in a module `coral.toml` lists under `[unsafe] modules`; that
the operand kinds are `in`/`out`/`inout` and the options are `volatile`,
`nomem`/`nostack`; that an operand is one machine word (integer, `bool`,
`char`, `f32`/`f64`, `Ptr<T>` or an `extern type` handle); that `out`/`inout`
name a place and that the place is writable (`var`, an exclusive borrow, a
`var` field); that a template's `{n}` names an operand that exists; and that a
`naked fun` takes no parameters, is not a method, and has exactly one `asm`
statement as its body. Both backends emit the same constraint string, the same
clobber list and the same operand order, so an asm that builds on one builds
on the other.

**What it does not check.** Anything about the template: what it does, what
registers or memory it touches, whether a clobber is complete, whether the
constraints fit the target, or whether the asm returns. There is no asm
verification and no `unsafe` proof (roadmap D-1/D-2/D-3 stay open); this stage
is the point where the language hands over the machine and says so.

**What the borrow checker sees.** An `in` operand is a read; an `out`/`inout`
place is taken as a transient exclusive borrow, so a conflicting live borrow is
refused. The operand types are `Copy` machine words, so no move or drop
bookkeeping is owed, and the borrow checker does not model the asm's own
memory effects (the clobber list is the only thing that tells the compiler
about them, and it is not checked).

## Stage 6: kernel 0 integration (`examples/kernel0`, `examples/kernel0-trap`)

**New ways to reach UB.** This stage adds no construct; it composes stages 1-5
into an image a boot loader starts, and every new way to be wrong is a promise
the compiler cannot see:

- The linker script and the generated `_start` are one contract split across
two files. QEMU's `-bios none` starts the image at 0x80000000 rather than at
the ELF entry, so any section the script does not name that lld places first
(`.eh_frame` here) silently puts `_start` somewhere QEMU does not jump to; the
kernel then executes data. `__stack_top` over program data or the vector table
is the stage-4 failure, unchanged.
- The trap vector is an address written with `la` against the module's mangled
symbol inside an asm template. A renamed or re-mangled function points `mtvec`
at nothing, at data or at the wrong function, and nothing checks the name; the
handler also assumes the trap is the `ecall` it expects and resumes at
`mepc + 4` unconditionally, so a different cause (an illegal instruction, a
fault) is resumed at an address that is not the next instruction.
- The naked handler's save and restore list is a promise. A register it
clobbers but does not save corrupts the interrupted code after `mret`, with no
diagnostic; the compiler emits no framing around the body to catch it.
- `mret` assumes the trap came from machine mode with the CSR state the handler
expects. Nothing restores `mstatus` or `mcause`, and a nested trap lands on
the same vector with the same stack.
- MMIO is a claim about the hardware. The UART base, the LSR offset, bit 5 and
the THR offset are the driver's own constants; writing THR without polling or
polling the wrong bit is simply wrong, not checked.
- `.bss` is not zeroed by `_start`, and `kmain` here does not touch it. The
first static a kernel adds is read as whatever the loader left in RAM.

**What the compiler does check.** Everything stages 1-5 check (the unsafe
listing, the freestanding allowed set, the naked body shape, the asm operand
shapes, the manifest's entry and panic shapes), on both `check` and `build`.
Nothing about the script's contents, the address a mangled symbol resolves to,
the trap cause, the handler's save list or the hardware.

**What it does not check.** The linker script, the `la` symbol in an asm
template, the trap entry assumptions, the register save completeness, the CSR
or privilege state at `mret`, and every MMIO fact. These are the reasons the
next stages add `interrupt` attributes, stack guards and the panic semantics
of stage 10, and MMIO capabilities in stage 11 (`plan/kernel0-status.md` lists
them in order).

## Stage 7: the freestanding `str` surface (`plan/kernel0-status.md` gap 1)

**New ways to reach UB.** A freestanding program can now hold and walk a `str`,
which is a NUL-terminated literal in read-only data. The type still carries no
proof of that, and the compiler does not add one:

- `str.len` scans to the first zero byte, so the promise NUL-termination is
  carries is the whole operation. A `str` that is not terminated (only reachable
  through a user `extern fun` that returns one, since this mode has no `String`
  and no `View.to_string`) reads past its own end until it happens to meet a
  zero. `str.byte` bounds-checks against that same scanned length, so it
  inherits the same assumption rather than adding a check of its own.
- `str.ptr` is the address of the first byte, and reading through it is as
  unchecked as any `Ptr<T>`. Taking the address needs no `unsafe`; dereferencing
  it does, and nothing relates the pointer back to the `str` it came from, so a
  driver that keeps it past the literal's own lifetime has no diagnostic.
- The panic handler's parameter is a real pointer to a real message now, which
  is the point of the stage. Its body stays unchecked in the stage-4 sense: the
  compiler neither requires it to return nor to print, and a handler that
  returns leaves the check that called it with no continuation. `str.len` on a
  message is the same NUL scan, so a message missing its terminator has the
  same failure as any other `str`.

**What the compiler does check.** The five operations are the only `std/str`
functions a freestanding program may call, and `std/mem` may not be called at
all; every other function in either module is refused by name at the call site
(`freestanding.rs`). `str.byte`'s index is checked against the scanned length and
a failure calls the project's `[target] panic` handler with `str.byte: index out
of bounds`, distinct from the array, `Bytes` and `View` texts. The three raw
externs are lowered as byte loops inside the module, and the lowerings are
marked `optnone` so the target's own optimizer cannot rewrite the NUL scan into
a `strlen` call or the byte compare into a `memcmp`.

**What it does not check.** NUL-termination of any `str` the program did not
write itself (there is no such `str` today), the provenance or lifetime of a
`str.ptr` result, whether the panic handler prints or returns, and anything
about the device a driver hand the bytes to. The emission half of the allowed
set (`prune`) drops the rest of `std/str` and `std/mem` before inference rather
than trusting the optimizer to remove their bodies, because at `-O0` nothing
would: an unwanted body there is an undefined symbol at link time, not a
diagnostic.

## Stage 7 (core language): global `static` data (MK9-S4-M6)

**New ways to reach UB.** A `static` is one object with a stable address for the
whole program, so this stage hands out a second kind of shared, long-lived
storage beside `Ptr<T>`:

- A `static mut` is the language's audited shared state. `unsafe` is the whole
  check: two threads that touch the same `static mut` without an `Atomic` or a
  fence are a data race the compiler does not see, and a `static mut` written
  inside an `unsafe` block is no more sequenced than a `Ptr` write. The
  read-modify-write a kernel writes as `COUNTER = COUNTER + 1` is not atomic.
- `Ptr.from_mut(NAME)` and `Ptr.from_ref(&NAME)` hand out the same raw pointer
  stage 1 describes, now to storage that outlives every frame, so the stage-1
  rules (alignment, provenance, the pointee still being the type it is read as)
  apply unchanged and no lifetime can make the pointer dangle.
- An immutable `static` is emitted read-only (`.rodata`, or `.data` when it
  holds an `Atomic`), and a `static mut` is writable storage in `.data`/`.bss`.
  A program that reaches an immutable static through a `Ptr` built with
  `Ptr.from_addr` and writes through it is the ordinary type-punning UB, not a
  new one; the compiler checks that a *written* static is `static mut`, not that
  a pointer derived from one is used correctly.
- A `static` initialised to all zero is placed in `.bss`, and a freestanding
  image's `.bss` is zero only because QEMU's ELF loader zeroes the segment; the
  compiler's `_start` still clears nothing, so a real boot loader that does not
  zero `.bss` reads indeterminate values where `.bss` is expected.

**What the compiler does check.** That a `static`'s type is a Copy scalar, an
integer/`bool`/`char` array, a `datac` of those, or an `Atomic<T>`, and no heap
type, borrow, `View`, function value, `Ptr<T>` or `extern type` handle; that its
initializer is a constant expression, folded before codegen; that every access
to a `static mut` is inside `unsafe { ... }` or an `unsafe fun`, in a module the
manifest lists under `[unsafe] modules`; that an assignment names a `static mut`;
and that a cross-module reference goes through `pub`. Both backends emit the
same object and the same access, and the freestanding allowed set accepts a
static by construction, because a heap type cannot be one.

**What it does not check.** The sequencing of two accesses to a `static mut`
(no atomicity, no ordering, no race detection), the address a `Ptr` derived from
one holds, or anything about the linker script's placement of `.data`/`.bss`. The
`Atomic<T>` operations remain the only safe shared mutable, and this stage does
not extend the (still unpublished) memory model to a `static` and a `Ptr` over
the same bytes.

## Stage 8: the address of a function (`plan/kernel0-status.md` gap 2)

**New ways to reach UB.** This stage turns a function into a machine address, so
the program can carry code as data and jump to it without the compiler seeing a
call:

- `Ptr.from_fn(f)` is the real symbol address after linking, and its element is
  always `u8`: the type says "some byte address", not "the function `f`".
  Calling through it is out of scope, so the language offers no checked call and
  a program that jumps to the result is on its own for the ABI, the stack and
  the calling convention. A `Ptr<u8>` may be stamped into `mtvec`/`VBAR` or
  otherwise handed to hardware, and nothing checks that the address is code,
  that the target is entered with the right privilege and register state, or
  that the code it names is still there.
- The pointer is taken safely, outside `unsafe`, because the compiler already
  checked the *name*: it is a declared `fun`, or a `fun` literal whose body
  captured nothing. A closure carries an environment the code address cannot
  describe, so it is refused, as is a scoped function value. A generic function
  or one that reads a passdown is refused for the reasons stage A-4 already
  gives, and a gated function is refused because a pointer is not a call site.
- `align(N)` on a `fun` is a **promise** that the first byte sits at an
  `N`-aligned address. The backend emits the alignment, but a pointer to a
  function reached through a link, a cast or a script that overrides placement
  carries no such guarantee; writing it into a vector register is UB for the
  same reason a misaligned `Ptr<T>` read is.
- The example's handler reaches its helper through `mscratch` (`csrr` then
  `jalr`), a jump the compiler never sees: nothing ties the register's contents
  to `report`, and a trap taken before `install` runs jumps through whatever the
  register held.

**What the compiler does check.** That `Ptr.from_fn`'s one argument is of type
`fun(...)`, naming the reason for a closure, a scoped value, or any other type;
that it takes no type argument, because its result is always `Ptr<u8>`; and
that `align(N)`'s `N` is a power of two up to 4096, on a top-level `fun` and
not on an `extern fun` or a `const`. Both backends emit the same address: the C
oracle casts the function to `uint8_t*`, LLVM uses the function value itself as
a `ptr`, and both lower `align(N)` as the function's alignment
(`__attribute__((aligned(N)))`, `LLVMSetAlignment`).

**What it does not check.** That a caller treats the address as a function at
all, the signature or convention it is entered with, whether the bytes it names
are still mapped, and anything about the linker script that ultimately places
the function. `align(N)` is emitted but its result is not re-checked by the
compiler at runtime, and `Ptr.from_fn` does not make the resulting pointer
non-null, non-wrapping or unique.

## Stage 8 (core language): `defer` (MK9-S6-M1, roadmap B-8)

**New ways to reach UB or unsequenced behaviour.** `defer` adds no new operation:
its action is an ordinary expression, duplicated at each ordinary exit of the
block that holds it, so every check stages 1-7 apply is applied to every copy.
What it adds is a *second execution site* for code the author wrote once, and
two ways to be surprised by it:

- **A panic does not run a deferred action.** A deferred lock release is skipped
  when a bounds check, an overflow check or an explicit `panic` fires: the lock
  stays held, the frame's owners are not freed, and the process ends. The
  compiler's panic path is a call, not an unwind, so the cleanup the `defer`
  promised never happens. This is a leak and a deadlock, not UB, and it is the
  one behaviour a reader of `defer` most wants stated.
- **The action runs once per dynamic exit, not once per program.** A `defer` in
  a loop body runs on every iteration's exit, and a `defer` in a block that is
  entered again runs again. A deferred action with a side effect is therefore
  not idempotent by construction, and nothing checks that running it twice (in a
  loop) means the same thing as running it once.
- **A deferred action that itself panics or never returns replaces the exit it
  is running on.** The block was already leaving; the action can loop forever or
  take the process, so a cleanup path can hang or abort. The compiler does not
  check termination, and it refuses only the exits an action may not *write*
  (`return`, and `break`/`continue` the day they exist) plus a `?` in the same
  block.
- `defer` inside `unsafe` inherits every stage-1 rule: a deferred volatile store
  to an MMIO address is the same unchecked write at a second site, and a
  deferred `Ptr.free` is a double free if another path already freed the block.

**What the compiler does check.** That each duplicated copy is ordinary code, so
inference, the move checker and the borrow checker see it: reading a local moved
out before the exit is a use of a moved value (reported at the `defer`), and a
borrowed tail keeps its elided lifetime. That a deferred action names only the
locals already in scope where the `defer` is written, so it cannot resolve a
binding the `defer` never saw. That the block the `defer` is written in contains
no `?`, because `?` leaves the function and the lowering has no exit for it. That
a deferred action contains no `return`. Both backends run the same lowered code.

**What it does not check.** Whether a deferred release matches a matching
acquire, whether the action's prerequisites still hold at the exit, whether it
terminates, whether it is idempotent, or anything on the panic path. The lexical
`?` refusal is per block, so a `?` in a *sibling* block still skips nothing that
this block's `defer` owes; and the scope check is a lexical rule about names, not
a lifetime, so a `Ptr` a deferred action reads is exactly as unchecked as it was
at the statement.

## Stage 9 (core language): the global allocator hook (MK9-S5-M1, roadmap B-7)

**New ways to reach UB.** A project can now name its own `alloc`/`dealloc`
(and optional `realloc`) in `[target]`, and `Box`/`List`/`Map` call them. Every
allocation the containers make is now a promise the compiler takes on trust:

- The compiler computes `size` and `align` from the element type and passes them
  to the project's `alloc`, but it does not check that the pointer it gets back
  is `align`-aligned, is large enough, is distinct from every live block, or is
  writable. A misaligned block turns every aligned field access built on it into
  UB; an overlapping block is an aliasing bug the containers cannot see; a
  region that is too small is a buffer overrun.
- `dealloc` gets `(ptr, size, align)` with the same numbers the type implies, and
  the compiler does not check that the pointer came from `alloc`, that it is not
  still live, or that it has not already been freed. A double free or a
  use-after-free is the project's UB.
- With no `realloc`, growth is the compiler's own alloc + copy + dealloc, so a
  copy of `old` bytes runs against the old block: if that block is smaller than
  the size the container recorded, the copy reads out of bounds, and if `alloc`
  returns a block that overlaps the old one the copy is unordered.
- A null return is *not* UB: the generated `check_alloc` calls the project's
  panic handler with `"out of memory"`, so a failed allocation panics instead of
  dereferencing null.
- The allocator may never effect a free (a bump arena's `dealloc` is a no-op),
  which is a leak the compiler cannot see and the counting wrapper in
  `examples/kernel0-heap` only detects for its own test.

**What the compiler does check.** That `[target]` names `alloc` and `dealloc`
together or neither, and that `realloc`, when named, needs them; that each name
is a bare identifier and a non-`extern` `fun` in the entry module; that their
signatures are exactly `(usize, usize).R: Ptr<u8>`, `(Ptr<u8>, usize, usize)`
and `(Ptr<u8>, usize, usize, usize).R: Ptr<u8>`; that with no pair `Box`, `List`
and `Map` are refused with a message naming the two keys, while `View` and
`Thread` stay refused even with one (`String` and `Bytes` join the allowed set
in stage 13); and that a freestanding image built this way references no libc
symbol (`kelp_memcpy`/`kelp_memset` are module-private definitions, and the
boot graph was checked with `llvm-nm -u`).

**What it does not check.** Any property of the memory the allocator returns:
alignment, capacity, provenance, liveness, distinctness or thread-safety. It
does not require `dealloc` to use `size`/`align`, does not require `alloc` to
honour `align`, does not require the arena to be non-empty, and does not check
that the allocator terminates or is lock-free. It does not run the allocator:
the checks are compile-time, and a bad allocator is only observable at the boot
the project writes.


## Stage 9 (runtime): the freestanding runtime essentials (`plan/kernel0-status.md` gap 4)

**New ways to reach UB.** This stage puts two pieces of compiler-written
bare-metal behaviour under a program that still cannot check either:

- The `_start` clear loop zeroes `[__bss_start, __bss_end)` with an 8-byte
  store, before `sp` is set up for anything but itself. Both bounds are
  **linker-script promises**: the loop treats them as an aligned, ordered,
  in-RAM pair, and a script that assigns them to a region that is not RAM, not
  word-aligned, or overlapping the image, the stack or a device register range
  makes the entry write outside its own clear range. A script that omits them
  is not UB, it is silence: both resolve to 0 and the clear does nothing, so a
  kernel that expected its statics zeroed reads indeterminate memory instead
  (which *is* UB to read for anything but `u8`/`i8` under this language's
  rules, though the compiler does not model initialisation at all).
- The weak `memcpy`/`memmove`/`memset`/`memcmp`/`bcmp` bodies are ordinary code
  once the linker resolves them. A program that defines its own symbol with one
  of those names takes over every call LLVM emitted for that shape, so the
  replacement has to implement the libc contract for the sizes the program
  uses: a `memcpy` that does not copy, a `memcmp` that never returns a
  negative, or a `bcmp` whose "different" answer is not nonzero is UB or
  silently wrong wherever LLVM chose a library call. The overlap rule is the
  sharp edge: `memmove` may overlap and `memcpy` may not, so a project that
  replaces `memcpy` with an overlapping forward copy corrupts its own data.
  Sizes are `i64` on every triple (the width `declare_libc` already used), and
  a definition whose width does not match becomes a *different* symbol under
  LLVM's renaming rather than an error.

**What the compiler does check.** That the entry stub's two bounds are weak, so
a script without them still links; that every `mem*` body is emitted with
`nobuiltin`, so the target's loop-idiom pass cannot rewrite a loop into a call
to the function it is already in; and that each is weak, so a strong
definition elsewhere in the image wins. The clear loop's range, alignment and
placement are *not* checked, because a linker script is not the compiler's to
read.

**What it does not check.** Anything about where the script put the two
symbols, whether the cleared region is safe to write, whether the stack the
stub loads immediately after overlaps it, or the semantics of a `mem*`
replacement a project links in. The per-call contract is LLVM's to assume and
the linker's to trust; the compiler supplies a definition that meets it, and
nothing more.

## Stage 10: kernel 0 on a second architecture (aarch64)

**New ways to reach UB.** The `[target] triple` now selects an architecture, and
the compiler trusts it in places it cannot check:

- The triple is a promise about the ISA the image will run on and nothing more.
  `aarch64-unknown-none` produces a well-formed ELF64 AArch64 image whatever
  machine boots it; booting that image on riscv64, or the reverse, executes
  another ISA's bytes. The compiler checks only that the triple is one it has a
  `_start` stub and a target machine for; it never checks the emulator, the
  loader or the hardware.
- The aarch64 stub reaches its linker-script symbols with absolute
  `ldr x0, =sym` literals (`R_AARCH64_ABS64`), and the aarch64 target machine is
  created with the **small** code model, because LLVM refuses `medium` on
  AArch64 (`only small, tiny and large code models are allowed`). Every branch,
  literal load and absolute address the compiler and the assembler emit then
  assumes the image and its symbols fit that model's ranges. A script that
  scatters sections far enough apart, or links at an address the model does not
  cover, can make the entry's own constant loads reach the wrong address, which
  the linker does not diagnose and the compiler does not model.
- The clear loop is the riscv64 one in AArch64 instructions: `cmp`/`b.hs` is the
  same unsigned bounds test, and `[__bss_start, __bss_end)` is the same pair of
  linker-script promises. Everything stage 9 says about a wrong, unaligned,
  overlapping or device-mapped clear range holds unchanged; omitting the bounds
  is still silence (both resolve to 0, the loop runs zero times, and a kernel
  that expected its statics zeroed reads indeterminate memory).
- The device constants remain the driver's claim: the PL011 base `0x09000000`,
  `DR` at `+0x00` and `FR` bit 5 (`TXFF`) are a claim about QEMU's `virt`
  machine, and a `Ptr` write to any other address is unchecked device I/O.

**What the compiler does check.** That the manifest's triple names an
architecture with a `_start` stub (riscv64 or aarch64, refused by name
otherwise); that `initialize_target_for` loads exactly that backend; and that
the stub assembles, because it is emitted as the target's own module assembly
with that target's MC parser registered. The triple's truthfulness about the
machine, the layout of the linker script and the reach of the code model are not
checked.

**What it does not check.** That the booted machine is the triple's
architecture, that the image is loaded where the script says, that the clear
range is safe to write, or that the small code model's assumptions hold for the
script's placement. It also still cannot check anything about a driver's
register addresses, which remain `Ptr` writes the language marks `unsafe` and a
manifest lists.

## Stage 11 (kernel path row 8): per-CPU storage (MK9-S3-M3, roadmap B-11)

**New ways to reach UB.** This stage adds one value the language reads but never
produces, and it is the kernel's to get right:

- **The CPU index register is trusted.** `cpu_index()` is whatever is in `tp`
  (riscv64) or `TPIDR_EL1` (aarch64) at the moment it runs. The compiler never
  sets it and never checks it in a release build, so a kernel that boots without
  loading it, loads it with a value that is not the running hart's id, or lets a
  context switch forget to swap it, indexes the per-CPU array with the wrong
  number. What happens then is ordinary unsafe memory misbehaviour: a read or
  write of another hart's copy, or of memory past the array when the value is
  `>= cpus`, through a `Ptr` the compiler marks `unsafe`. In a debug build
  (`-O0`) the value is compared against the manifest's `cpus` and the project's
  panic handler runs with `cpu index out of range`; a release build carries no
  check, so the misbehaviour is silent. Nothing ties the register to a hart id:
  reading `mhartid` and loading `tp` from it is the kernel's own instruction
  sequence.
- **The register is per hart, and a value written under one hart's `tp` is not
  visible to another's copy by itself.** There is no memory-ordering or
  cache-coherency rule here beyond what the architecture and the `unsafe` code
  do; the padded layout only stops false sharing of a cache line, it does not
  synchronise. Two harts writing their own copies concurrently is fine; two
  harts reaching one copy is a data race exactly as two `Ptr` writes to one
  address always were.
- **The initializer is replicated, not run.** All `N` copies start as the same
  folded constant, so a `percpu` static cannot express a per-CPU initial value
  that differs by index (a `[target] cpus` of 4 gives four copies of 7, not
  0, 1, 2, 3). A kernel that wants that assigns it in its own boot code.
- The `.percpu` section, its 64-byte alignment and the padding that makes each
  copy start on a cache line are the compiler's layout, as reliable as any other
  global's; what is not checked is where the linker script puts the section,
  whether the image has room for it, and whether the address a `Ptr` is built
  from is one of the copies at all.

**What the compiler does check.** That a `percpu` static is `mut`; that it is
declared in a freestanding build; that its initializer folds to a constant, so
no code runs at startup to fill the copies; that naming it is inside
`unsafe { ... }` or an `unsafe fun` in a module the manifest lists; that
`[target] cpus` is an integer in `1..=64`; that `cpu_index()` and `cpu_count()`
are called in a freestanding build with no arguments; and that `cpu_index()` is
lowered for the manifest's triple (riscv64 and aarch64 say which register, any
other triple is refused by name). The object writer sets the section, the
64-byte alignment and the `round_up(size_of(T), 64)` stride, so the layout
itself is not a promise the kernel makes.

**What it does not check.** The value in the register, that it names the running
hart, that it is below `cpus` (release only), or that the register survives a
context switch. There is no per-CPU provenance model, so the compiler cannot tie
a `ptr<T>` built from a `percpu` copy back to the copy, exactly as it cannot tie
any `Ptr` to the object it came from (roadmap D-1).

## Stage 12 (core language): user `Drop` (MK9-S6-M1, roadmap B-8, second half)

**New ways to reach UB or unsequenced behaviour.** A destructor is an ordinary
method body, so every check of stages 1-11 applies to it; what the stage adds is
a *second execution site* the author did not write at the value's death, and
three ways to be surprised by it:

- **A destructor runs at a drop point the author may not see.** A value moved
  into a `List` or a closure, or one a `pd` owns, is dropped where that owner
  dies, which can be another function's frame or process exit or every loop
  iteration. A destructor with a side effect is therefore not idempotent by
  construction, and nothing checks that running it once per drop point means the
  same thing as running it once; a `defer` and a `Drop` on one value run twice
  if the author writes both.
- **A panic does not run a destructor**, for the same reason it does not run a
  `defer`: the panic path is a call that ends the process, not an unwind. Every
  value the abandoned frame owns leaks, and a lock a destructor would have
  released stays held. This is the same admitted leak `plan/mk9-memory-model.md`
  §2 records for an abandoned worker frame, now stated for every frame.
- **A destructor that itself panics or never returns replaces the drop point it
  is running on.** The compiler does not check termination, idempotence, or that
  the body leaves the value in a state its field drops can handle. A destructor
  that drops a value of its own type (directly or through a container it builds)
  recurses on the C stack exactly as a recursive `datac` drop always did, so a
  cycle of destructors overflows the stack rather than being refused.
- **A destructor reads its passdowns as globals.** The generated
  `kelp_drop_<T>` calls the body from every drop point with one pointer, so a
  destructor cannot take a hidden passdown parameter; the body reads the
  generated global directly, the same read-only borrow `main` takes. There is no
  exclusivity check across a destructor, so a destructor that writes a `static
  mut` (inside `unsafe`) is the same unchecked write any other function's is.

**What the compiler does check.** That a destructor is exactly one per type,
declared as `fun mut T.drop()` on a user `datac`/`enum`: an exclusive receiver,
no parameters, no return value; that a builtin type and a generic template
cannot declare one; that it cannot be called explicitly (`g.drop()` is refused
by name); and that a destructor type is not `Copy` by excluding it from the
derived deep-copy and key fixpoints, so `List.clone`/`slice`, `Map.get_or`/
`values` and a declared key pair are refused rather than producing a second
owner of the resource. Both backends put the user call at the top of the same
generated `kelp_drop_<T>`, so every existing drop point reaches it identically.

**What it does not check.** Whether the destructor releases the right resource,
whether it matches its acquisition, whether it terminates, whether it is
idempotent, whether two destructors over one object are ordered safely, or
anything on the panic path. The value is still live (borrowed exclusively) while
the body runs, so the body cannot move out of it, but it can assign a field or
read through a raw `Ptr` the value holds, with all of stage 1's consequences.
## Stage 11: Sv39 address spaces (MK9-S4-M8, `plan/address-spaces.md`)

**New ways to reach UB.** This stage adds no construct. Every line of
`examples/kernel0-vm` is stage 1's `Ptr<u64>`, stage 7's `static mut`, stage 5's
`asm` or ordinary arithmetic, so the compiler's whole check set is the one
stages 1-10 already apply. What is new is that the machine state those unchecked
writes build is the translation itself, and the ways to be wrong are the ways a
page table can lie:

- The frame region is a **linker-script promise** read with
  `asm("la ...")`. A script that puts `__frames_start`/`__frames_end` outside
  RAM, overlapping the image or the stack, or describes a region that is not
  actually mapped lets `alloc_frame` hand out live memory as a fresh frame, and
  the walker then writes PTEs over whatever was there. A script that omits the
  two symbols is not UB but silence: both resolve to 0, `next + PAGE_SIZE >
  frames_end()` is true immediately, and the allocator reports exhausted (which
  the example turns into a panic line, not a link error).
- `alloc_frame` returns a bare address. Nothing checks it is RAM, 4 KiB-aligned,
  not already handed out, or that the `FRAME_CURSOR` arithmetic has not wrapped.
  "Zeroed on allocation" is the allocator's own promise, and only the code that
  reads a table before writing every entry depends on it.
- Every PTE and `satp` value is the program's claim about the format. A wrong
  PPN, a missing A or D, a leaf bit where a pointer entry belongs, an entry that
  points at a device page instead of RAM, or a root that was never zeroed all
  produce a translation the hardware acts on and the compiler cannot read. A
  leaf written with a misaligned PPN silently becomes a 2 MiB or 1 GiB superpage
  covering far more than the caller meant.
- `enable_paging` writes `satp` and one `sfence.vma`. A store to a page-table
  entry is not ordered against the MMU's own read of it, so a mapping change
  without the flush leaves a stale translation, and the compiler emits none
  itself. The root the CSR names is the caller's address, checked by nothing.
- `map_page` does not check that the virtual address is canonical for Sv39
  (bits 63:39 must equal bit 38). A non-canonical address builds an entry the
  MMU will never use or faults on, and the failure is at the access, not the
  call.
- The drop to S-mode is four CSR writes and an `mret`. `pmpaddr0`/`pmpcfg0` are
  the only reason S-mode can touch memory at all; `medeleg` decides which
  exceptions reach the S-mode vector and which come back to machine mode, where
  this kernel installs nothing; `mstatus.MPP` decides the privilege `mret`
  returns to; and `mepc` is the address it jumps to. The compiler models none of
  this: a wrong MPP returns to the wrong privilege, and a wrong `mepc` jumps
  anywhere.
- The S-mode handler is a naked function, so stage 8's register-save promise
  and stage 6's handler assumptions apply verbatim. Its resume path advances
  `sepc` by the faulting instruction's own length, read from the low bits; a
  wrong length resumes mid-instruction, and resuming past a faulting load leaves
  the destination register holding its old value while the compiler that emitted
  the load assumes the loaded value is there. Using that value after a resumed
  fault is the miscompile the handler cannot see.
- `translate` walks the tables and returns a physical address. It says nothing
  about whether the leaf's permissions allow the access a caller intends, and
  nothing about whether the address is RAM or a device range: it is the walk's
  answer, not an access check.

**What the compiler does check.** Everything stages 1-11 apply: that `vm` and
`main` are on the manifest's `[unsafe] modules` list; that a `Ptr<u64>` element
has a machine-level layout; that `asm` operands are machine words inside
`unsafe`; that a `static mut` read or write is inside `unsafe`; and the
freestanding allowed set. It checks no new construct because the stage adds
none.

**What it does not check.** Anything about the linker script's frame region,
the address `alloc_frame` returns, the PTE or `satp` encoding, the canonicality
of a virtual address, the reachability of a physical one, the privilege
transition, the handler's save list or resume length, or the meaning of a
translation. This stage is the point where the kernel builds the machine's own
view of memory with the language's unchecked tools, and the catalogue for those
tools is stages 1, 5, 7 and 8.

## Stage 12: user mode and the syscall ABI (MK9-S4-M9, `plan/syscall-abi.md`)

**New ways to reach UB.** No new construct: `examples/kernel0-user` is stage 1's
`Ptr`, stage 7's `static mut`, stage 5's `asm` and stage 11's page tables. What
is new is that the kernel now hands the machine's control flow and its address
translation to code it does not trust, and the ways to be wrong are the ways a
privilege boundary and a trap frame can lie:

- The trap frame's layout is a **convention between two languages**: the naked
  entry hardcodes 0, 8, 248 and 256, and the `datac` the dispatcher reads says
  the same in its field order. The compiler relates neither to the other, so a
  field reordered in the `datac` saves or restores the wrong register. The
  stage's two checks (`trap.frame_layout_ok` at boot, and
  `tests/projects/trap_frame_layout` on both backends) pin the numbers, and
  neither can see an asm offset typed as a different literal.
- The slot block at the top of the trap stack is addressed by four more
  hardcoded offsets. A wrong dispatch slot calls an arbitrary address with the
  frame in `a0`; a wrong resume pair jumps to an arbitrary address with an
  arbitrary stack. Nothing checks that the slots were ever written: a hart that
  enters U-mode without `enter_user` has a stale dispatch address and a stale
  resume pair.
- `__trap_stack_top` is a linker-script promise read with `asm("la ...")`. A
  script that omits it resolves it to 0, so the first trap swaps `sp` with 0 and
  the entry faults on its own frame.
- The entry swaps `sp` with `sscratch` **unconditionally**, because the only
  trap source it installs is U-mode. The invariant is "`sscratch` holds this
  hart's trap stack top whenever user code runs", and an S-mode fault breaks it
  the same way a wrong `sscratch` value would: the handler runs on whatever
  stack the swap produced, overwrites the frame it is building, and the resume
  path restores the wrong `sp`. There is no check and no second stack to fall
  back on.
- The drop to U-mode is three CSR writes and an `sret`. `sstatus.SPP` decides
  the privilege it returns to and `SPIE` what interrupt state that privilege
  gets, `sepc` is the address it jumps to, and `satp` is the table the fetch
  after it is translated by. The compiler models none of this: a wrong `satp`
  enters a program under another program's tables, and one whose root does not
  identity-map the kernel image faults on the instruction after the switch.
- `enter_user`'s return path is a **jump into a saved stack frame**, not a
  return. The resume slots hold the kernel's `sp` and `ra` from the call into
  `drop_to_user`, and `trap_entry` restores them by hand. The compiler cannot
  see the call or the jump, so nothing checks that the saved frame is still the
  one on the stack, that the trap stack was not the stack it saved, or that the
  program which ended is the program that started.
- The dispatcher reads `a0`..`a5` and `a7` out of the frame at hardcoded
  offsets, and writes the result back to `a0` and `sepc + 4` the same way. A
  wrong offset passes the wrong argument to a syscall or puts the result in the
  wrong register. The `+ 4` assumes the faulting instruction is a 4-byte
  `ecall`, which is true of an `ecall` and of nothing else.
- The untrusted-pointer check is deliberately **one purpose, not a type
  system**. `vm.user_page_pa` demands U and R, so it answers for a page a user
  may read; it says nothing about whether the page belongs to the program that
  asked, whether two programs share it, whether the leaf is a superpage, or
  whether the physical address it returns is RAM rather than a device. It also
  answers about a snapshot: a remap between the walk and the copy would move
  what the physical address means, and this kernel has one hart and a handler
  that never remaps, which is the only reason the two steps are one.
- The kernel dereferences the **physical** address the walk returned, not the
  user address, and that is what makes the check meaningful. A leaf whose PPN
  points at the kernel's own image, the trap stack or the UART makes `write`
  read and print whatever is there while every check still passes.
- The errno values are bit patterns a user program reads as negative numbers.
  Writing `-14` in Kelp and converting it with the compiler's own `isize.to_u64`
  is a **checked** conversion that panics on a negative value, which the first
  boot did: the panic handler ran inside the trap handler and the kernel halted.
  The ABI is therefore written as the two's complement constants. The same
  checked arithmetic is why `write`'s range bound is `ptr > USER_LIMIT - len`
  and not `ptr + len > USER_LIMIT`: an adversarial pointer near the top of the
  address space would otherwise panic the kernel instead of earning `EFAULT`.
- The user programs are a **section copied at runtime**. Nothing checks that the
  section fits the page the loader maps, that the program is position
  independent, that its own `la` references survive the copy, or that the
  linker script's `__user1_start`/`__user1_end` (and the `__user2` pair) name the
  sections they are supposed to. A script that omits them makes both bounds 0,
  the copy moves nothing, and the program's code page is zeros: an illegal
  instruction the kernel reports as a kill.
- A program's address space is built once and never freed. A killed program's
  frames stay allocated, and nothing marks a frame as belonging to a program, so
  a second program's tables can point at the first's pages only if the builder is
  changed to do so; there is no allocator state that would notice.

**What the compiler does check.** Everything stages 1-12 apply: that every
module here is on the manifest's `[unsafe] modules` list; that `asm` is inside
`unsafe` and a naked body is exactly one `asm`; that `static mut` reads and
writes are inside `unsafe`; that a `Ptr` element has a machine-level layout; and
that a conversion that can lose a value is checked rather than silent (the point
above, where the check is what the kernel had to design around). It checks no
new construct because the stage adds none.

**What it does not check.** The frame's offsets against the asm's, the slot
block's, the trap stack's address, the value of `sscratch`, which privilege
`sret` returns to, which table `satp` names, the saved kernel `sp`/`ra`, the
register offsets the dispatcher uses, what a validated user pointer is
*for*, what the physical address it names contains, or whether a copied program
is still the program that was assembled. The catalogue for the unchecked tools
is stages 1, 5, 7, 8 and 11.

## Stage 13 (row 7): freestanding `String` and `Bytes` (MK9-S5-M2)

**New ways to reach UB.** With an allocator configured, `String` and `Bytes`
become legal, and every buffer the two types grow is memory the project's own
allocator returned through the same promise stage 9 already took on trust. The
buffer layer adds four shapes stage 9's containers did not have:

- `kelp_buf_reserve` computes the new capacity as `want = cap == 0 ? 8 : cap`
doubled until it holds `len + extra`, and the **capacity is the only record of
how large the block is**: `dealloc` is handed `cap + 1` bytes. A project whose
`alloc` returned a smaller block than the `size` it was given, or whose
`realloc` returns a block that is not `want + 1` bytes, turns the next append or
the free into an out-of-bounds access the compiler never checks. The reverse is
a leak, not UB.
- The `realloc` fast path passes the project `(ptr, cap + 1, want + 1, align)`.
An `alloc`-and-copy replacement is expected; a `realloc` that moves the block
**without** telling the compiler is fine only because the buffer reloads the
pointer it stores, but one that copies fewer than `cap + 1` bytes, or that
returns a block overlapping the old one, is silent corruption. With no
`realloc` named, the fallback is the compiler's alloc + copy + dealloc, and the
copy overlap rule is stage 9's.
- `String.from_str` and `Bytes.from_str` derive the length with the compiler's
own NUL scan and then copy `n + 1` bytes. A `str` without a terminator in its
allocation makes the scan read past the literal, before any allocation happens.
The bodies carry `optnone` so the scan cannot become a `strlen` call, which is a
*promise the compiler keeps*, not something the program can break.
- `Bytes.get`/`set` and `str.byte`'s bounds checks call the project's panic
handler; a handler that returns instead of looping lets execution continue past
the check into the code the check was guarding, which is UB if the index was out
of bounds. Nothing requires the handler to diverge.

**What the compiler does check.** That `String`/`Bytes` are refused without an
allocator, with the message naming the two keys; that every method not on the
libc-free list (and every `std/str` function outside the listed subset,
including `str.from_f64`) is refused at the call with "needs the hosted
runtime", before either backend runs; that every allocation and free the two
types make reaches the project's `alloc`/`dealloc`/`realloc` with `align = 1`
and size `cap + 1`, and that a null return reaches the panic handler with
`"out of memory"`; and that the image references no libc symbol (`llvm-nm -u`
stays clean, and the IR carries no `malloc`/`realloc`/`free`/`strlen` call).

**What it does not check.** Any property of the memory the allocator returns,
exactly as stage 9 does not; whether `realloc` grew the block, copied all
`cap + 1` bytes or kept the new block disjoint from the old; whether the `NUL`
terminator a `str` was promised is inside its allocation; and whether the
project's panic handler diverges. The buffer's byte loops are ordinary code
once the module is verified: the compiler supplies them and trusts the
allocator, and nothing more.

## Stage 13 (row 7): arenas and a fallible allocation surface (MK9-S5-M3)

**New ways to reach UB.** This is the other half of row 7. `Ptr.try_alloc` hands
back uninitialised memory through the same promise `Ptr.alloc` already took on
trust, with one difference: failure is the null `Ptr<T>` rather than a panic, so
the null check the compiler used to make for the program (the `check_alloc`
branch) is now the program's own. The arena adds two more shapes:

- `Ptr.try_alloc<T>(count)` returns memory the allocator claims is `count * size`
  bytes. Reading a slot before writing it is the stage 1 indeterminate-value
  rule, and the byte count is checked for *wrap* on both backends (a count whose
  product would overflow `usize` is the null pointer, so a huge request cannot
  silently become a small block). What is unchecked is the rest: that the memory
  is large enough, that the returned address is not null before it is
  dereferenced, and that `count` items are ever written.
- The alignment promise is the allocator's. Hosted, `Ptr.try_alloc<T>` is
  `malloc`, which is aligned for `max_align_t` and not necessarily for an
  over-aligned `T` (`repr(align(64))`); treating an under-aligned address as
  that `T` is the stage 2 UB. Freestanding, the manifest's `alloc` is handed the
  type's own alignment and its answer is taken as the promise.
- `std/arena` never checks or records anything about `base`: the region has to be
  live, writable and at least `cap` bytes for as long as the arena hands out
  pointers, and `Ptr.byte_offset` is unchecked arithmetic, so nothing ties a
  returned pointer's provenance to the region. `reset` frees nothing and clears
  nothing, so every pointer handed out before it is a use-after-reset the
  compiler cannot see (the bytes are still there; reading or writing them is the
  stage 1 rule, not an error). `alloc_zeroed` writes zeros *through* the returned
  pointer, so a wrong or null `base` is a write to an arbitrary address, the
  worst shape in this catalogue.
- A `datac` field of `Ptr<T>` (which `Arena` has) is now laid out by the LLVM
  backend, so a `Ptr` can be a struct member. Nothing about the field is checked
  either: copying the `datac` copies the address, and the address carries no
  lifetime, no provenance and no tie to the region it was derived from.

**What the compiler does check.** That `Ptr.try_alloc` is an unsafe operation
(inside `unsafe { ... }` or an `unsafe fun`, and in a module the manifest lists
or the compiler ships on its own list); that its element is a type with a
machine-level layout; that `count * size` is refused for wrap on both backends;
that `std/arena`'s `align` is a power of two (a zero or non-power value panics
with the module's own message) and that the padded end is computed with `%` and
`/` and refused at each step that could wrap, so `alloc` answers the null pointer
rather than a short block; that the freestanding lowering reaches the manifest's
own `alloc` (so `llvm-nm -u` stays clean) and that the call is refused by name
without one; and that the `unsafe` block in `std/arena` is admitted only through
the compiler's short `stdlib::UNSAFE_MODULES` list. It checks nothing about the
memory any of it returns.

**What it does not check.** The allocator's honesty, the region's size, liveness
or alignment, the null check before a dereference, the provenance of any address
`Ptr` arithmetic builds, whether a pointer is used after `reset`, or whether
`alloc_zeroed`'s write lands inside the region. D-1/D-2/D-3 stay open; this stage
adds the two allocator shapes they will have to describe.

## Stage 14: stack guards and probing (MK9-S7-M2, the stack half)

**New ways to reach UB.** The compiler now emits a page probe on every
freestanding function, and the kernel supplies the hole the probe is for:

- **The guard page is the kernel's, not the compiler's.** `probe-stack` only
  turns a large frame into a sequence of stores one page apart; whether those
  stores land on an unmapped page is the linker script's business. A script that
  puts the guard above the stack, sizes it to zero, or maps it anyway means the
  probe touches ordinary RAM and the overflow is silent. `examples/kernel0-stack`
  splits its identity map around `[__guard_bottom, __guard_top)` to leave the
  hole, and the two bounds are linker-script promises the compiler cannot read.
- **The probe is not a check.** It does not compare `sp` against a bound, does
  not know a stack exists, and does not run on a `naked` function (no prologue)
  or on a function whose frame is one page or less (nothing to probe). A frame
  smaller than a page can still cross the guard with its ordinary stores, which
  is why the small-frame recursion faults too; a frame that is exactly the stack
  size plus a little may put its first probe store past the hole and its body
  into mapped memory below it.
- **The fault stack has to be separate and mapped.** A trap handler that runs on
  the stack it just overflowed faults on its own frame, and the S-mode handler
  here runs on a 16 KiB region below the guard with `sscratch` naming its top.
  A `sscratch` that is not set, not switched, or points into the hole is the
  same class of failure the stage documents for `kernel0-user`; a guard page
  below the trap stack would make the handler's own overflow equally quiet.
- **The resume path is a jump, not a return.** After the first fault the handler
  points `sepc` at the second phase, resets `sp` to `__stack_top`, restores
  `sscratch` and `sret`s. Nothing checks that the phase function is a function,
  that `__stack_top` is the top of the stack the kernel meant, or that the
  interrupted frame's registers are the ones the resumed code expects; the
  abandoned phase's frames are leaked in place.
- **The recursion shape is the demo's promise.** LLVM may tail-call-eliminate or
  accumulator-rewrite a recursion into a loop on one frame, and may collapse a
  recursion it can prove non-terminating into an infinite loop; either makes the
  stack growth vanish with no diagnostic. The example's volatile store after the
  recursive call and its volatile `limit` are what keep the recursion real, and
  they are the program's own defence, not a guarantee the compiler makes.

**What the compiler does check.** That every non-declaration function of a
freestanding LLVM build gets the `probe-stack="inline-asm"` string attribute and
that a hosted build gets none; that the attribute's value is the one LLVM
lowering understands; that the example's `asm`, `static mut` reads and writes and
`Ptr` accesses are inside `unsafe` in listed modules; and the whole freestanding
allowed set. Nothing about the frame size at which the probe fires, the pages it
lands on, the linker script's bounds, or the fault stack's address.

**What it does not check.** Whether the guard page exists, is unmapped or is
below the stack; whether the trap stack is separate, mapped and large enough;
whether `sscratch` holds its top; whether the resumed `sepc`/`sp` are the ones
the kernel meant; whether a recursion was rewritten into a loop; and whether the
platform delivers the fault to the handler at all. D-1/D-2/D-3 stay open; the
guard page and the fault stack join the catalogue of promises a kernel makes
with `Ptr`, `static mut`, `asm` and a linker script.

## The typed `std/tls` handle (release goal 1 follow-up, `plan/tls-handle.md`)

Not a kernel stage, recorded here because it changes what an opaque handle can hide.

**New way to reach undefined behaviour.** `TlsHandle` is an `extern type`: C lowers it to `void*` behind
`kelp_tls_TlsHandle` and LLVM to `ptr`, and every OpenSSL body trusts it (`(SSL*)kelp_handle`). A handle
that is null, already freed by a `try_shutdown` that ran twice, or produced by a different library is
undefined behaviour inside OpenSSL, exactly as the old `usize` `0` or a stale integer was. What the type
buys is that a handle cannot be manufactured from an integer, compared with one, or confused with a
different handle type, so the *accidental* integer-to-pointer path is gone. It does not track liveness:
`TlsHandle` is `Copy`, owns nothing and has no drop, so nothing stops a handle outliving the `SSL_free`
that released it.

**What the compiler does check.** That `extern type` is declared in a module with an `[extern.*]` section
or a module `stdlib.rs` ships; that a handle is used only as itself (`==`/`!=` with a handle of the same
or an ancestor type, an argument, a binding, a return, `null()`/`is_null()`/`cast()` only in an extern
module); and, in `std/http`/`std/ws`, that the plain/TLS choice is an `Option`, so no integer sentinel can
stand in for a handle.

**What it does not check.** Whether a handle points at a live `SSL` object, whether it was freed once,
whether the module that produced it is the one that consumes it, or whether the C body's cast is the right
type. Those are the promises `std/tls`'s header documents and the AddressSanitizer gate exercises.

## Stage 16: MMIO as a capability (MK10-S1, roadmap C-3, `plan/device-caps.md`)

**New way to reach undefined behaviour.** None from the check itself: it only
adds a compile-time rule about which addresses `Ptr.from_addr` may name, and a
call it admits is as unchecked at every later step as it was before. What the
domain *moves* is the honesty of a grant: an `mmio` range is the compiler's
record of "this module was allowed to name this address", and a program that
takes a granted address, offsets past the range (`p.byte_offset(n)`,
`p.offset(n)`, `p.cast<U>()`) and reads or writes it is doing exactly what
`Ptr` always allowed, now with a paper trail that stops at the pointer's birth.
The `["*"]` grant is the escape hatch: it admits a computed address, so a module
that holds it can reach any address the integer arithmetic yields, and it is the
line to grep for when auditing a kernel's device surface.

**What the compiler does check.** That `mmio` is a capability domain and a
grant in it is a list of half-open ranges (`"0xBASE+0xLEN"`, hex or decimal,
non-zero length, pairwise disjoint) or the wildcard alone; that a freestanding
module's `Ptr.from_addr` holds a grant at all; that a constant address falls
inside a granted range; and that a computed address is only accepted under
`["*"]`. The check runs in the front end before either backend is named, so
`kelpc check` and `kelpc build` agree; it is a no-op for a hosted project and
for the C oracle, because freestanding mode is LLVM-only by design.

**What it does not check.** Whether the granted range is really memory-mapped,
really a device, or really this project's; anything about an address derived
from a granted one by arithmetic, a cast or a read of the tick counter; whether
two modules' ranges overlap or whether a module's range is correct for the
machine it will boot on; and, in the other direction, whether a module with a
correct-looking grant actually needed it. There is still no runtime check, no
provenance model (roadmap D-1) and no ownership: a granted range says nothing
about who may write it, when, or through how many pointers at once.

**Grants the examples carry.** The UART drivers name a compile-time constant
and hold exactly their device's window: `0x10000000+0x100` for the riscv64
NS16550 (`kernel0`, `-trap`, `-heap`, `-panic`, `-percpu`, `-statics`, `-string`
and the two freestanding arena/`try_alloc` projects) and `0x09000000+0x1000`
for the aarch64 PL011. `kernel0-bss` (the BSS marker's address comes back from
`asm`), `kernel0-stack`, `kernel0-user`, `kernel0-vm` and `kernel0-vm-fault`
grant `["*"]` to the modules that build pointers from computed addresses (a
page-table frame, a trap-frame slot, a user page, the faulting `sepc`), because
a non-constant address is refused under a range grant by design.

## Language bugfix: mutation of the place a `for` walks (2026-10-02, not a kernel stage)

**New way to reach undefined behaviour.** None, and one path to memory exhaustion is closed. The
`for` desugar re-reads `xs.len()` every pass, so `for x in xs { xs.push(x) }` on a `Copy` element
(where nothing borrowed the list) grew the list until the allocator failed, which is a clean panic
rather than UB but took the machine down on 2026-10-01. The body may now read the iterated place
and change a different one, but not mutate the one it walks.

**What the compiler does check.** That a `mut` receiver method on the iterated place (`push`,
`pop`, `remove`, `insert`, `set`, `clear`, `sort`, `reverse`), an assignment to it, a move of it,
`&mut xs`, an argument to a `mut` parameter, and a `fun` literal that captures it are all one
front-end refusal, `cannot mutate 'xs' while iterating over it`, on the `for` line. The comparison
is the place's canonical path, so `for x in self.items { self.other.push(x) }` is legal and a
written `while` over an index is untouched.

**What it does not check.** The rule is name-based and looks at a statement's own place, so it
misses an indirect write, a passdown, and a raw-pointer alias. An indirect write is a call on
something else that reaches the field, such as `for x in self.items { self.grow() }` where `grow`
pushes to `self.items`; that is not detectable without effect analysis, and a `Copy`-element list
can still grow that way. A `pd var` list is read through a global's storage, not a local name, so
the path recorded for `for x in xs` (a `pd var`) does not match the write's place and the rule does
not fire; a freestanding project has no heap types at all, so this is hosted-only. A raw-pointer
alias is a `Ptr<List<T>>` in a module an `unsafe` block lets write through it, which the type
system does not track here. A shadowed binding of the same spelling
inside the body is refused as if it were the iterated one (over-strict, the conservative
direction). And it says nothing about a list mutated by another thread while a `for` walks it,
which is the `Atomic`/lock story and not this rule. (An exclusive borrow taken *before* the loop,
`val p = &mut xs` then `p.push(..)` in the body, is already refused by the lexical borrow rule: the
desugared `.len()` needs a shared borrow of `xs` and the alias holds it exclusively.)

## Language feature: `==`/`!=` over a declared `eq` (H-5, 2026-10-02, not a kernel stage)

**New way to reach undefined behaviour.** None. The operator is removed before
borrow checking and code generation: `a == b` becomes the ordinary call
`a.eq(&b)` (and `a != b` becomes `!a.eq(&b)`), and `View` against `View`/`str`
becomes `View.eq`. Both backends see a call of a declared function, so there is
no new lowering, no new pointer arithmetic and no new memory access. The
operands are the already-checked expressions from `check_binary_operands`, so a
side-effecting operand is evaluated once and left to right, exactly as the
written call would.

**What the compiler does check.** That the type declares `fun T.eq(other: &T).R:
bool` (the shape A-12's key-pair validation already enforces, so a lone `eq`
with the wrong signature is an error at its declaration), and that the operand
types are the same `datac`/`enum`, or a `View` against a `View`/`str`. A `datac`
or `enum` with no declared `eq` keeps the old `cannot compare` refusal, nothing
is derived from fields, and `<`/`<=`/`>`/`>=` are refused on every such type. A
lone `eq` gives the operator and no `Map` key: the map still needs both `hash`
and `eq`, so a keyable type with a declared `eq` is hashed and compared by the
*derived* pair unless it declares both.

**What it does not check.** Nothing new beyond what the declared `eq` body
already does: the operator is only as sound as that body, and there is no
requirement that it be reflexive, symmetric or transitive (a `Map` with such an
`eq` can fail to find a key it inserted, which is a logic bug the compiler does
not see).

## Language feature: traits with static dispatch (A-12 slice 2, 2026-10-02, not a kernel stage)

**New way to reach undefined behaviour.** None. A trait is a front-end declaration: it adds no
type, no vtable, no witness argument and no new `tir` node, so there is nothing for a backend to
lower and nothing for a program to reach at run time that it could not reach before. An `impl`
block's methods are ordinary receiver methods on the type, and a bound on a type parameter is
checked where the instantiation is created and then disappears: the emitted call is the direct call
the concrete type's method already was. A freestanding image proves it
(`tests/projects/freestanding_min` uses a trait and still links with no undefined symbol, no libc
and no runtime helper), and the trait implementation in that image is a plain direct call.

**What the compiler does check.** That a trait is declared once with method names that are unique
and are not `drop`; that a signature's receiver is `Self` and it has no body; that an impl is the
only impl of that trait for that type, lives in the trait's module or the type's, names a declared
`datac`/`enum`, and implements exactly the declared signatures with the declared receiver `mut`,
parameter names and types, and return type; that an explicit method of the type does not share a
name with a trait method; and that every type argument at every call/instantiation implements every
bound on its parameter, transitively. `trait Drop` is refused: `fun mut T.drop()` stays the one
destructor spelling, and a type with a destructor keeps every rule stage 12 gave it.

**What it does not check.** Nothing about the *meaning* of an implementation: a trait method's body
is checked like any other method's, so an `area` that is not an area, an implementation that
panics, and two traits implemented for one type that disagree all compile. A method call on a
parameterised `T` resolves against the type each instantiation binds rather than against the
bound's own signature list, so a call to an inherent method of the concrete type also compiles
everywhere the bound holds; the bound guarantees the trait's methods exist, it does not restrict
the body to them. There is no `dyn`, no default method, no associated type and no operator
overloading through a trait, each refused where it is written.

## Language feature: `Mutex<T>` in `std/sync` (B-5's remainder, 2026-10-02, not a kernel stage)

**New way to reach undefined behaviour.** The mutex itself adds none at the language level: `lock`
hands the block a `mut T` borrow the existing scoped-block rules refuse to store or return, and
`into_inner` moves the value out, so no two paths reach the guarded value unsynchronized. The new
surface is the runtime record (`kelp_rt.c`: a `pthread_mutex_t`, a poison flag, the `T` slot and
`T`'s stored drop pointer), and the one way to get it wrong is a value copied into the record and
then dropped by its old owner as well: the checker's `Mutex.new(value: move T)` argument is the
ordinary moved argument, so the caller's binding is marked moved and its drop is a no-op.

**What the compiler does check.** That `T` is owned and borrow-free (`is_mutex_element`, the
`Send`-shaped `!is_borrowed_field_ty`, with `unit` and a function value refused), that a `Mutex` is
not a container element, that the block's `v` cannot escape, that the receiver of `into_inner` is
consumed, and that a freestanding manifest refuses the whole type by name.

**What it does not check.** Nothing about what a block does with `v` beyond the borrow rules: two
`lock` calls in one thread deadlock, a program can deadlock on lock ordering, and poisoning is a
runtime flag the compiler cannot see. A panic inside the block releases the lock and poisons the
mutex through the runtime's `pthread_cleanup_push` handler, but the panicking frame's owned values
are still abandoned (`plan/mk9-memory-model.md` section 2). The runtime's cleanup is only reached
on the `pthread_exit` path a worker takes; a panic on the main thread still ends the process with
`exit(1)`, so a mutex it held is never unlocked and never observed.

## Stage 15: panic semantics with no OS (MK9-S6-M2, row 10's other half, `plan/panic-semantics.md`)

**New way to reach undefined behaviour.** A check that fires now calls a compiler-emitted entry
(`kelp_panic_entry`) instead of the project's handler directly, and the entry reads two module
globals (`kelp_panic_file`, `kelp_panic_line`) that the *statement before it* wrote. Three new
promises follow from that:

- The location is a single pair of globals, not an argument to the check. Two harts that panic
  concurrently can overwrite each other's site between the store and the read, so a double panic on
  one hart may name the other's line. The flag itself is an atomic exchange, so the *re-entry*
  decision is sound; only the reported line is not.
- A panic inside the handler is detected by that flag and handed to `kelp_double_panic`. The
  compiler emits a weak definition (a `wfi`/`wfe` loop), so an image that does not override it
  wedges rather than printing; a project that overrides it takes over every double panic, including
  a handler that recurses for a reason other than a check (a bad pointer, a second `panic`).
- `kelp_double_panic` is reached with whatever stack the second panic left, and the compiler
  generates no second stack or recovery point. A handler that loops forever in a double panic is
  the same failure as before, one level down.

**What the compiler does check.** That the handler is one of the two accepted shapes
(`fun(msg: str)` or `fun(msg: str, file: str, line: u32)`) with no environment and no return, and
refuses any other by name. That every check's message is the compiler's own text and every location
is a real `SourceMap` line, on both `kelpc check` and `kelpc build` (the check is in the front end).
The `.bss` flag and the two location globals are cleared by the `_start` clear loop like any other
`.bss` byte.

**What it does not check.** Whether the handler returns, prints, or reads the location: a handler
that returns still comes back to a check that has no continuation. Nothing ties a location to the
address the check actually read, and the generated accessor bodies that read the globals
(`Array.get`'s `__bounds`, the container sets) are shared by every call site, so their line is
whichever statement last ran, not a property of the body. `plan/panic-semantics.md` has the check
by check table and names the two constructs the task lists that are *not* checks here: integer
overflow wraps and division by zero is unchecked UB, on this backend and the C oracle alike.

## Stage 18: IRQ lines and physical frames as capabilities (MK10-S1, roadmap C-3, `plan/irq-frame-caps.md`)

**New way to reach undefined behaviour.** None from the check itself, exactly as stage 16: a
`Line` and a `Frame` are values, and a check that admits a call says nothing about the machine. What
the two domains *move* is where the grant's honesty stops. An `irq` line is a number, and nothing
ties it to a controller, a mask bit or an installed handler, so `irq.claim(10)` may name a line that
does not exist and the compiler will not say so. A `Frame` is a 4 KiB-aligned address, and
`Frame.offset(byte)` bounds the byte against the frame's own 4096, but the `usize` it returns is
ordinary arithmetic from then on: a caller that walks on with `+ n` is outside the frame and the
compiler does not revisit it, which is stage 16's "born-at only" limit one level down. A frame
handed to a device for DMA is not followed at all: the module may free it, reuse it, or hand out a
second `Frame` for the same address, and nothing sees the conflict. `["*"]` is the same escape hatch
it is for `mmio`: a module that holds it can claim any line or any aligned address.

**What the compiler does check.** That `irq` and `frames` are capability domains and their grants
are ranges (`irq` a decimal line or inclusive line range, `frames` a `"0xBASE+0xLEN"` window, both
pairwise disjoint and never beside `["*"]`); that a module holds the domain before `irq.claim`,
`frame.claim` or any accessor of either module is reachable; that a constant line or frame falls
inside a granted range; that a constant frame address is 4096-aligned whatever the grant; that a
computed line or address is admitted only under `["*"]`; and that a `Line`/`Frame` constructor
cannot be written from another module (the capability type is sealed by name, `stdlib::SEALED_TYPES`,
because a shipped module exports every item and the language has no field-level `pub`). The check is
in the front end, so `kelpc check` and `kelpc build` agree on both backends and in both the hosted
and the freestanding modes; the one runtime check is `Frame.claim`'s 4096-alignment test, which
fires only for a computed address.

**What it does not check.** Whether the line exists, is enabled, is shared, or has a handler;
whether the frame is RAM, device memory, already owned by another module, or still mapped when a
device writes to it; anything about a byte past `Frame.offset`'s returned number; and, in the other
direction, whether a module with a correct-looking grant actually needed it. There is still no
runtime ownership, no liveness, no provenance (roadmap D-1) and no DMA tracking. `examples/kernel0-caps`
carries the one grant the boot uses (line 10 and `0x80100000+0x100000`), and `plan/irq-frame-caps.md`
is the design note.

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
and `Map` are refused with a message naming the two keys, while `String`,
`Bytes`, `View` and `Thread` stay refused even with one; and that a freestanding
image built this way references no libc symbol (`kelp_memcpy`/`kelp_memset` are
module-private definitions, and the boot graph was checked with `llvm-nm -u`).

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

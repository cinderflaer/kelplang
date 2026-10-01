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


## Stage 9: the freestanding runtime essentials (`plan/kernel0-status.md` gap 4)

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

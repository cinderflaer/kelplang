# Stage 5: inline assembly and naked functions (MK9-S7-M1/M3, roadmap B-9, B-16)

*Design for kernel-path stage 5. `plan/kernel-path.md` is the context; this file says what is
built and what is deliberately left out.*

## What this adds

One expression and one declaration clause, and they are the two halves of the same thing: a
trap vector is a naked function whose whole body is one `asm`.

- **`asm("<template>", <operands...>, <options...>)`** is an expression, and therefore also a
  statement. It is legal only inside `unsafe { ... }` or an `unsafe fun`, so the module that
  writes one is listed in `coral.toml`'s `[unsafe] modules` exactly as any other `unsafe`
  construct is (`plan/unsafe-and-pointers.md`).
- **Operands**, in order of appearance:
  - `in("<constraint>") expr`: read `expr` into the asm, an input operand;
  - `out("<constraint>") place`: write the asm's result into `place`, an output operand;
  - `out("<constraint>") _`: an unnamed output, see the value form below;
  - `inout("<constraint>") place`: read `place` in and write the result back to it;
  - `clobber("<register>")`: the asm writes a register the compiler may otherwise keep
    something live in.
- **Options**: `volatile` (accepted, and the default: Kelp has no pure asm, so the instruction
  is always emitted with LLVM's `sideeffect` and GCC's `volatile`), `nomem` (the asm does not
  read or write memory, so the default memory clobber is dropped), and `nostack` (accepted;
  see "what the compiler does not check").
- **`naked fun name()`** declares a function with LLVM's `naked` and `noinline` attributes
  (`__attribute__((naked, noinline))` on C). Its body must be exactly one `asm(...)`
  statement and nothing else; nothing else is emitted either, so no prologue, no epilogue and
  no compiler-generated `ret`. A naked function plus an `asm` is a trap vector.

## The dialects, and why the template is not portable

The template is the *target assembler's* text, and the operand placeholders are `{0}`, `{1}`,
... in order of appearance (the first operand is `{0}`). The compiler translates them to the
backend's own spelling (`$0` for LLVM, `%0` for GCC) and escapes the character that would
otherwise be read as a placeholder (a literal `$` becomes `$$` on LLVM; a literal `%` becomes
`%%` on C).

The dialect is the one the target's own tools default to, and it is chosen by target so that
one template reads the same way everywhere:

- **x86-64 and x86**: Intel syntax (`LLVMInlineAsmDialectIntel` on LLVM, and a
  `.intel_syntax noprefix` / `.att_syntax prefix` wrapper around the template on C). This is
  the syntax that writes the destination first (`add {0}, {1}` accumulates into `{0}`,
  `mov rax, 42` puts 42 in `rax`), which is the only spelling under which the first operand,
  the output, is also the first thing written.
- **every other target** (riscv64 first): the target's one syntax, LLVM's default dialect
  (`csrr {0}, mstatus`, `add {0}, {0}, {1}`). riscv64 writes the destination first too, so the
  same convention holds.

The consequence is stated rather than hidden: an `asm` template is per-target, and a program
that wants two targets writes two templates behind a compile-time choice it does not have yet.

## The value form of `out`

`out("r") place` and `inout("r") place` write their place; they are not values. The only
value-producing form is **one anonymous `out("...") _`**, which is the expression's value:

```
val mstatus: i64 = unsafe { asm("csrr {0}, mstatus", out("r") _) }
```

The anonymous output's type is the context's when there is one (`val m: i64 = ...`) and `i64`
otherwise, because an output is a machine word. A second `_` is an error rather than a guess,
and `in` never produces a value.

## Lowering

Both backends build the same two lists, and the observable behaviour is the same:

- **constraint order**: every `out` and `inout` first, in appearance order, then every input
  (`in`, and `inout`'s own tied input), in appearance order. An `inout` is one output
  constraint plus an input tied to it (`=r` and `0` on LLVM, `+r` on GCC). `clobber("x")` adds
  `~{x}` on LLVM and `"x"` in GCC's clobber list; `~{memory}`/`"memory"` is added unless
  `nomem` was written.
- **operand numbers**: an `out`/`inout` is numbered by its position among the outputs; an `in`
  is numbered after the outputs. LLVM counts an `inout`'s tied input as an operand of its own,
  GCC does not, so the mapping is computed per backend and the two produce the same assembly.
- **the call**: `LLVMGetInlineAsm` builds a value of a generated function type whose return is
  the single output's type (or a literal struct of them, or `void`), and `LLVMBuildCall2` calls
  it. `sideeffect` is set. Each output is then stored to its place, and an `inout` loads its
  place as the tied input first. The C backend emits one `__asm__ volatile(... : outputs :
  inputs : clobbers)`.
- **naked**: `LLVMAddAttributeAtIndex` at the function attribute index with the `naked` and
  `noinline` kinds. The body is the asm call followed by `unreachable`, because the asm itself
  is what returns; on C the body is the asm statement alone.

## Gating

`asm` is an `unsafe` operation, so `ast::first_unsafe` reports it exactly as it reports an
`unsafe` block, and `resolve.rs`'s `[unsafe] modules` check is what makes a module that writes
one legal. That list is stage n's (MK9-S2), which is present on this branch, so the gate is the
existing one rather than a new equivalent. A `naked fun` is a declaration, not a block, so it is allowed wherever a `fun` is
allowed; it is the `asm` inside it that carries the listing. A `naked fun`'s own body is an
unsafe context (like an `unsafe fun`'s), because its one statement is an `asm` and no other
reading of it exists. `asm` and `naked` are contextual words, not keywords: `asm` introduces
the asm form only when a `(` touches it, `naked` declares a naked function only when `fun`
follows the run of markers, and both stay usable as ordinary names everywhere else.

## What is deliberately not here

- **`interrupt` and calling-convention attributes** (`interrupt`, `naked`-adjacent
  `interrupt_handler`, syscall/fastcall conventions, stack alignment control). Out of scope
  for this stage. A naked function whose body is one `asm` covers a trap vector without them:
  the vector is the naked function, the asm saves what it needs and ends in the trap return
  instruction, and the compiler emits nothing around it. What is missing is the *attribute*
  that tells LLVM the function is an interrupt handler so it can adjust its own reasoning; a
  naked function sidesteps that reasoning entirely, which is why it is the smaller thing to
  build first.
- **Target feature detection, SIMD intrinsics, cache hints** (the rest of B-9). Unrelated to
  the asm spelling.
- **Multiple targets for one template.** See the dialect section.
- **A constraint vocabulary the compiler understands.** `"r"` and `"rax"` are passed through
  as LLVM constraint names; the compiler checks the operand *shape*, never that the register
  exists or that the register and the operand's type agree. That is the assembler's job, and it
  is a compile error there (GCC) or a link/assembly error (LLVM), not a language error.
- **`asm` in a freestanding project's allowed set**: it is allowed, because that is the mode a
  kernel is built in and the naked function plus asm is the whole reason this stage exists.

## What the compiler does not check

The section that matters for `plan/kernel-ub-notes.md`: the compiler checks that the operand
kinds are `in`/`out`/`inout`, that an operand is a place of a machine-word type, that the
places are writable, and that the construct is inside `unsafe`. It does not check the template
against the constraints, the constraints against the target, the clobber list against reality,
that `nostack` is true, that a naked function's asm returns, or that an `asm` that writes a
register neither it nor a clobber names leaves the program in the state it intended.

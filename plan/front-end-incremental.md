# Front-end incrementality (A-14 slice 4)

*A design record. It follows A-14's shipped backend-unit cache and is not
implemented by that cache.*

## Decision

The compiler will cache **generated module interfaces**, keyed by checked source
and configuration. Interface files are compiler artifacts, never source files a
user writes or commits. A module that imports `util` may reuse its own checked
front-end result while `util`'s implementation changes, provided `util`'s
generated interface hash is unchanged.

This is deliberately module-granular: parse, resolve, type inference and
borrow checking remain one unit of reuse. There is no query engine, AST cache
shared across compiler versions, or cross-module inlining in this slice.

## Interface contents

An interface serializes the checked public contract of one module:

- module id, exports and their visibility;
- exported function, `extern fun`, `datac`, `enum`, trait, constant and static
  signatures, including generic parameters, bounds and associated items;
- the imported module ids and the interface hash each one supplied;
- capability domains a module reaches, and each declared import delegation;
- compiler and language-format versions, target, build flags, manifest values
  that can affect checking, and the hashes of shipped stdlib sources it imports.

It contains no function body, local type, private item, source text, diagnostic
span, or generated LLVM. A private implementation edit therefore keeps an
interface stable; an exported signature, grant, import, feature flag or stdlib
change invalidates its dependents.

The compiler writes the artifact atomically under the existing cache root. A
miss, malformed artifact, compiler-version mismatch or hash mismatch is a fresh
check, never an error and never a best-effort reuse. The artifact has a versioned
binary header and a canonical ordering for every collection, so an identical
project produces an identical interface hash.

## Build algorithm

1. Discover and parse the module graph as today, retaining each module's source
   hash and direct imports.
2. Walk dependencies before importers. For a module, validate its generated
   interface cache key from its source hash, checking configuration and direct
   dependency interface hashes.
3. On a hit, load its checked public contract. On a miss, resolve, infer and
   borrow-check that module against its dependencies' contracts, then atomically
   store its generated interface.
4. Assemble the typed program from the checked module results, then run the
   existing backend-unit pipeline unchanged.

Diagnostics remain source-backed: a cached provider supplies only a public
contract, while a changed module is checked against its own live source. A
diagnostic that needs a dependency's declaration location causes that dependency
to be loaded from source for the diagnostic, rather than serializing stale spans
into a cache artifact.

## Namespace decision

`std/internal/...` is reserved for compiler-owned library implementation. User
projects may not declare or import that root, including through an alias. Shipped
public modules remain `std/...`; an internal module is reachable only from a
compiler-shipped module and is absent from `stdlib::shipped()` and public
documentation. This keeps implementation dependencies out of user code without
adding a second package root or changing existing `std/...` imports.

## Tests and acceptance

- A private body edit recompiles that module and its backend unit but reuses a
  direct importer's front-end result.
- An exported signature, imported module, capability delegation or target/build
  setting invalidates every direct dependent, and transitively any importer whose
  generated interface then changes.
- Corrupt, old-version and partial interface files are misses and are replaced
  atomically.
- A user `impl "std/internal/x"` and a user source file below `src/std/internal/`
  are refused by name.
- A clean incremental build and a forced-cold build produce the same diagnostics,
  program output and backend-unit IR.

The first implementation may expose only `--timings` hit/miss lines. A cache
inspection CLI and remote cache sharing are later work; neither changes the
correctness rule that an interface is derived from checked inputs.

## Design pass 2026-10-06: what the code forces

Read against `resolve.rs`, `infer.rs::check_program` and `borrowck.rs::check_program`.
The decision above stands; these findings change how it is built and what an
interface must hold.

### Findings

1. **The front end is flat, not modular.** `resolve::load` merges every module
   into one `ast::Program` with prefixed names, and `infer::check_program`
   returns one `tir::Program` (`structs`, `enums`, `pds`, `functions`, `caps`,
   `gated`, `declared_keys`, `drop_types`, ...). There is no per-module result
   to retain today. The unit of reuse has to be a *function group per module*
   spliced into a rebuilt program, not a module-shaped checker.
2. **`tir` has no serializer, and `kelpc` has no dependencies.** A restored
   checked body needs a canonical binary form of `tir::Function` (about 37
   `ExprKind`/`Stmt` shapes plus `types::Ty`). That is the largest single piece
   of work and must land first, with a round-trip test over every fixture.
3. **A body edit can leak across a module boundary through four channels the
   current `api_hash` does not see.** The hash is built from `doc::scan`
   headers, so it covers signatures only:
   - generic templates: an importer holds *instances* of a dependency's generic
     function, so the template body is part of the importer's checked output;
   - passdown modes: `check_program` runs a probe pass and a second pass with
     `close_pd_modes`, a whole-program fixpoint over which functions write a
     passdown. A dependency body that starts writing one changes its callers'
     borrow checking;
   - `const` initializers, which are folded into importers;
   - drop and destructor sets (`drop_types`, `owned_types`), which are
     program-wide and feed `borrowck`.
   An interface therefore carries, beside signatures: the body hash of every
   generic template, the final passdown mode of every public function, folded
   const values, and the module's contribution to the drop set. Without these a
   reuse hit can be wrong, which breaks the cold/warm identity rule.
4. **Borrowck consumes the whole `tir::Program`** (`fn_borrow_root`,
   `fn_mut_params`, enums by name). Its per-function results (the drop plan)
   must be cached with the body, keyed the same way, or borrowck stays
   whole-program and only inference is reused. Slice it that way first.

### Revised order

1. `tir` binary codec with round-trip tests (no behaviour change).
2. Extend the interface hash with the four channels in finding 3, each with a
   test that a body edit which should leak does invalidate the importer.
3. Interface validation on load: format version, source and configuration
   identity, direct dependency hashes; failure is a miss, replaced atomically.
4. Per-module function-group cache for `infer`, keyed by module source hash,
   configuration and the dependency interface hashes; splice into the program;
   `borrowck` stays whole-program.
5. `--timings` front-end hit/miss lines; cold/warm identity test over the whole
   fixture suite (diagnostics, output, unit IR).
6. Only then consider caching `borrowck` per function.

Hotspot note: steps 1 and 4 touch `tir.rs` and `infer.rs`; run one lane at a
time while they are in flight.

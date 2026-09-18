# X compiler — passdown

Status snapshot for picking this back up.

## Current state: syntax rewrite is DONE and committed

The pivot described in earlier versions of this doc is complete: `xc`
now implements `syntax&shii.md`'s grammar end-to-end (lexer → parser →
type-checker → C codegen), not the old `fn`/`let`/`struct`/`println`
grammar. `cargo test` is green: 31 unit tests (lexer + parser) + 10
end-to-end fixture tests (`xc/tests/compile_and_run.rs`, which compiles
`.x` fixtures with the real `xc` binary and checks stdout / rejection),
zero warnings.

Commits, in order:
- `0269405` `plan.md` (original milestone plan, old syntax — now stale
  on syntax, milestone *structure* still roughly the reference).
- `08f192e`/`ca4af6a`/`4c9f914` — old-syntax M0/M1/M2 (superseded by the
  rewrite below, kept in history).
- `7932cab` — threaded `Span` through `tir::Expr` (still valid, needed by
  the still-unimplemented move checker, see below).
- `aa8221e` — earlier version of this passdown doc, written mid-rewrite.
- `1b616c5` — **the syntax rewrite**, current HEAD. Full lexer/parser/
  types/infer/codegen_c rewrite onto `syntax&shii.md`'s grammar.

### What the rewrite actually implements

- **Newline-terminated statements, no semicolons.** Lexer tracks
  paren-depth; emits one `Newline` token per run of blank lines only
  when depth is 0 (so a `datac`'s field-list parens, or any parenthesized
  sub-expression, can span multiple lines — but nothing else can: an
  expression that needs to span lines *must* be wrapped in parens, no
  Kotlin-grade trailing-operator continuation was attempted). Parser
  requires a `Newline` or `}` after each statement (`parse_block`), and
  the same rule for `when` arms.
- **`val` / `var`** replacing `let` / `let mut` (`ast::Stmt::Let` kept its
  shape, just relabeled semantically).
- **`fun` + `.R: T` return clause** (parsed as `.` then a literal `R`
  token then `:` then the type — parser rejects anything else there) +
  **expression-bodied functions** (`fun f(...).R: T = expr`); using `=`
  without a `.R: T` clause is a parse error (no cross-function return-type
  inference). An expr-body is lowered to `tir::Block { stmts: [], tail:
  Some(expr) }`, same shape a block-bodied function's implicit-return tail
  produces.
- **`datac Name(val/var field: T, ...)`** replacing `struct Name { field:
  T }` — fields declared in the constructor-style parens (so no separate
  brace body), each individually `val` or `var`. **Real per-field
  mutability**, not just root-binding mutability like the old M2: a `val`
  field is never reassignable no matter how the containing value is
  bound; a `var` field is reassignable exactly when both (a) the root
  binding is `var` and (b) that specific field is `var`. See
  `infer.rs::check_assign`.
- **Construction via call syntax**: `Point(x = 1, y = 2)` — no separate
  struct-literal grammar. `ast::Expr::StructLit` doesn't exist anymore;
  `infer.rs::check_call` disambiguates a `Call`'s callee by checking the
  struct table before the function table (and `check_program` rejects a
  name used for both). This also *removed* the old
  struct-literal-vs-block-in-condition ambiguity problem from M2 (no
  `no_struct_lit` parser flag anymore — nothing to be ambiguous with).
- **Named + positional call arguments together**: `ast::CallArg { name:
  Option<String>, value: Expr }`; parser treats `ident =` (exactly, not
  `==`) as a named-arg prefix, only when unambiguous by adjacency.
  `infer.rs::match_args` is the one shared resolver (by name first,
  remaining params filled positionally in declared order) used by both
  plain function calls and `datac` construction. `types::FnSig` now
  carries parameter *names*, not just types, because of this.
- **`print(x)` / `print!(x)`** replacing `println`/`print`. **The
  polarity flipped**: `print` now appends the newline, `print!` doesn't
  — opposite of the old naming, easy to trip over if muscle memory says
  otherwise. Lexer folds the specific adjacent shape `ident!(` (identifier
  immediately followed by `!` immediately followed by `(`) into one
  identifier token, rather than merging `!` into identifiers generally
  (which would wrongly eat `!=` written as `x!=y` with no spaces).
- **Compound assignment** (`+=`, `-=`, `*=`, `/=`, `%=`) — new lexer
  tokens, desugared at parse time to `target = target OP value`
  (`parser.rs::parse_assignment`).
- **`when { cond -> expr, ... else -> expr }`** — pure parser sugar,
  desugars immediately to a nested `if`/`else if`/`else` `Expr::If` chain
  (right-to-left fold in `parser.rs::parse_when`). `infer.rs` and
  `codegen_c.rs` never see a `when` node at all — zero changes needed
  there for this feature.
- **`&str` type annotations parse** (`parse_type` accepts and discards a
  leading `&`) — there's no real reference/borrow type yet (that's
  borrow-checker territory, still deferred, see M3/M4 below), but the
  spec's examples write the string type as `&str` everywhere, so the
  annotation needed to at least parse into plain `Ty::Str`.

### A real bug the rewrite surfaced (now fixed)

Expression-bodied functions (`fun f(...).R: T = expr`) exposed a
pre-existing gap in `codegen_c.rs`: a function body block's *tail*
expression (the implicit-return value) was being silently discarded as a
bare statement instead of becoming a C `return`, because the only
codegen path for a block's tail (`write_block_body`) is the
"used-as-a-statement, discard the value" one — correct for `if`/`while`
bodies and nested `{ }` blocks, wrong for a function's own top-level
body. This never surfaced under the old grammar because every old
fixture used explicit `return` statements. Fixed by adding
`write_function_body` (turns a non-`Unit`-typed tail into `return
<value>;`; a `Unit`-typed tail, including `main`'s, is still discarded
as before) and using it only for a function's own body, not nested
blocks. Symptom before the fix: garbage return values (e.g. a `.R: &str`
expression-bodied function returning an uninitialized pointer → segfault
on `print`). Worth remembering if a similar "value silently
discarded instead of returned" bug shows up elsewhere in codegen.

### Explicitly deferred (not implemented), with reasons — unchanged from before the rewrite

- **Receiver-form methods** (`fun Circle.area()`, `fun mut Circle.grow()`)
  — `syntax&shii.md`'s own "M1" delta, not M0; drags in
  method-resolution-by-receiver-type plus the real
  implicit-shared-vs-`mut`-exclusive-borrow rule (old M4 territory).
  Functions are free functions only for now.
- **`pd` passdowns** — the doc's own §9 leaves the mutable-global/
  lifetime story as an open question with three unresolved options.
- **`impl` multi-file imports** — not even parsed yet. Real loading was
  always M5 in `plan.md`.
- **String templates** (`"$var"`, `"${expr}"`) — doc's own §12 lists this
  under "M2" delta, not M0. Only plain (non-interpolated) string literals
  exist.
- **`caps(...)` capability syntax** — depends on `impl` multi-file.
- **`stdlib.md`** (repo root) — separate, much larger companion doc about
  stdlib namespaces/`Bytes`/capability wiring. Out of scope until asked.

### M3 (ownership/moves) — still not implemented, plan below is unchanged and still valid

No `borrowck.rs` exists. Plan (syntax-independent, still applies):
Copy-vs-non-Copy rule (primitives are Copy, `datac` values are not);
move-state tracked per-scope the same way `infer.rs` tracks variable
scopes; a bare `Ident` read of a non-Copy value moves it; a `.field`
read of a Copy-typed field does not move the containing value (only a
bare whole-value `Ident` read does, under the current grammar, since
there's still no way to take a reference in expression position). Spans
are already threaded through `tir::Expr` (see `7932cab`) specifically so
move-checker diagnostics can be Rust-style "value moved here" / "used
here" pairs.

**Bonus for when M3 resumes**: `datac`'s per-field `val`/`var` model
(built properly in this rewrite, not deferred) is exactly the kind of
per-field bookkeeping partial-move reasoning will eventually want, so
this rewrite wasn't a detour from M3 — it's groundwork for it.

## Recommended next steps

1. M3 (moves) on the new syntax, using the plan above.
2. M4 (borrowing) — needs real `&`/`&mut`/deref support in expression
   position, which the parser doesn't have yet (only in type-annotation
   position, and only as a no-op `&` prefix accepted for `&str`).
3. Only then reconsider the deferred syntax features above, prioritized
   by what's actually needed to keep writing example programs (probably
   `impl` multi-file next, since a real program wants more than one
   file — then receivers, since methods are used pervasively in
   `syntax&shii.md`'s own examples).

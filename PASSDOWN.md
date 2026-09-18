# X compiler — passdown

Status snapshot for picking this back up. Written mid-pivot: the surface
syntax is being rewritten (see "Syntax pivot" below) and that work is not
yet started in code.

## What's implemented and committed (old syntax: `fn`/`let`/`struct`/`println`)

Commits on `main`, in order:
- `0269405` — `plan.md` (original milestone plan, old syntax).
- `08f192e` — **M0**: `xc` crate scaffold. Hand-written lexer with spans
  (`lexer.rs`), recursive-descent + Pratt parser (`parser.rs`) for
  functions/`let`/arithmetic/comparison/logical exprs/`if`-`else` (statement
  and expression)/`return`/calls, pretty-printer (`pretty.rs`). `xc build`
  parses and pretty-prints only, no codegen yet.
- `ca4af6a` — **M1**: `types.rs` (primitive `Ty`), `infer.rs` (typed-IR
  checker with scopes, span-carrying errors), `codegen_c.rs` (typed IR → C,
  using GNU statement-expressions `({ ... })` for `if`/block-as-value, with
  a temp-variable pattern for `if` specifically since C statement-exprs
  can't yield a value straight out of `if`/`else` control flow), `driver.rs`
  (shells to `cc -O2`). `xc build`/`xc run` go source → native binary.
  `-o <path>` build flag. End-to-end harness `tests/compile_and_run.rs`
  (compiles fixtures with the real `xc` binary, runs them, checks stdout).
- `4c9f914` — **M2**: `struct` decls, field access (incl. chains), `let mut`
  + assignment expressions (to bindings and struct fields), `while` loops.
  Mutability tracked as a property of the *root binding* only (no per-field
  mutability — that changes under the new syntax, see below). Struct-literal
  vs. block-in-condition ambiguity resolved the way Rust resolves it
  (`parser::no_struct_lit`, reset inside brackets).
- `7932cab` — Threaded `Span` through every `tir::Expr` (was previously only
  on the AST), because M3's move-checker needs "value moved here" / "used
  here" span pairs and spans didn't reach the typed IR yet.

Test count at last commit: 22 unit tests (`lexer.rs`, `parser.rs`) + 7
end-to-end fixture tests (`tests/compile_and_run.rs`), all passing, zero
warnings.

**M3 (ownership/moves) is NOT implemented** — no `borrowck.rs` exists yet.
I was mid-way through planning it (Copy-vs-non-Copy rule: primitives are
Copy, `struct`/`datac` are not; move-state tracked per-scope like `infer.rs`'s
variable scopes; a bare `Ident` read of a non-Copy value moves it, a
`.field` read of a Copy-typed field does not) when the syntax pivot
happened. None of that is written down anywhere else — this paragraph is
the only record of that plan.

## Syntax pivot (in progress, no code changes made yet)

`syntax&shii.md` (repo root) is a from-scratch rewrite of the surface
syntax, explicitly superseding `plan.md`'s syntax while `plan.md`'s
*milestone structure* stays the reference for now ("Reconciling plan.md is
a separate later edit" — per that file). Headline changes from what's
built:

| Old (built) | New (`syntax&shii.md`) |
|---|---|
| `fn`, `->` return type | `fun`, `.R: T` return type, after the param list |
| `let` / `let mut` | `val` / `var` |
| `struct Name { field: T }` | `datac Name(val/var field: T)` — fields individually `val` or `var`, declared in the constructor param list, not a brace body |
| `Name { field: value }` literal | `Name(field = value)` — construction reuses call syntax with named args |
| `println(x)` | `print(x)` (newline) / `print!(x)` (no newline) |
| Semicolons | Newline-terminated statements, no semicolons at all |
| `use "path"` | `impl "path" [with caps(...)] [as alias]` |
| (none) | `when { cond -> val, ... else -> val }` — replaces if/else-if chains |
| (none) | `pd val`/`pd var` — passdowns (implicit globals wired only into functions that reference them) |
| (none) | Receiver-form methods: `fun Circle.area().R: f64 = ...`, `fun mut Circle.grow(...)` — replaces `impl` blocks and `self` |
| (none) | Named arguments at call sites (required when 2+ params) |
| (none) | String templates: `"$var"`, `"${expr}"` |

The user chose (via AskUserQuestion) **"stop and rewrite lexer/parser now"**
over continuing M3/M4 on the old grammar, or resolving the doc's own open
questions first.

### Scope I had settled on for this rewrite pass, before being interrupted

Because `syntax&shii.md` itself flags unresolved conflicts with
`plan.md`'s milestones (its §12/§14) and leaves some semantics genuinely
undecided (`pd` mutability/lifetime story, §9's "open question"), I was
not planning to implement the whole doc in one shot. Decisions made so far
(not yet coded):

**In scope for this rewrite:**
- Newline-sensitive statement termination, no semicolons. Planned
  mechanism: lexer suppresses `\n` tokens while paren-depth > 0 (so a
  `datac`'s field-list parens, or any parenthesized sub-expression, can
  span lines); at depth 0 it emits one `Newline` token per run of blank
  lines. Parser treats `Newline` as the block/top-level statement
  separator (replacing `;`), and — since a statement's "is this a tail
  expression" status no longer needs a `has_semi` flag — that field on
  `ast::Stmt::Expr` goes away entirely; "textually last in the block" is
  now the *only* rule (this actually simplifies `infer.rs`, which already
  used that rule as the deciding factor). Explicitly **not** attempting
  real Kotlin-grade ASI (trailing-operator line continuation outside
  parens is not supported — an expression that needs to span lines must
  be wrapped in parens).
- `val` / `var` replacing `let` / `let mut`.
- `fun` keyword, `.R: T` return-type clause, `return expr`, and
  expression-bodied functions (`fun f(...).R: T = expr`) as sugar over a
  single-statement block. A `=`-bodied function without `.R: T` is a parse
  error (no cross-function return-type inference).
- `datac` replacing `struct`, with **real per-field mutability**: a field
  declared `val` is never reassignable regardless of the binding's own
  mutability; `var` fields are reassignable when the root binding is
  `var`. This is a genuine improvement over the old "mutability is only a
  property of the root binding" model from M2 — replace it, don't layer it.
- `datac` construction via call syntax (`Circle(r = 2.0)`) — no separate
  struct-literal grammar. This also *removes* the old
  struct-literal-vs-block ambiguity problem in `if`/`while` conditions
  (`parser::no_struct_lit`) since there's no more `Name { ... }` literal
  form to be ambiguous with a block.
- Named and positional call arguments together (`ast::Expr::Call` gets
  `Vec<CallArg { name: Option<String>, value: Expr }>`); resolution matches
  named args by name against the callee's declared param names (so
  `types::FnSig` needs to start carrying param *names*, not just types),
  fills remaining params positionally in declared order. Same matcher used
  for `datac` construction against its field list.
- `print(x)` / `print!(x)` replacing `println`/`print`. Lexer detects the
  specific adjacent shape `ident!(` (identifier immediately followed by
  `!` immediately followed by `(`) and folds it into one identifier token
  `"print!"`, rather than generally merging `!` into identifiers (which
  would wrongly eat the `!=` operator when written without spaces,
  e.g. `x!=y`).
- Compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`) — new lexer tokens,
  desugared in the parser to `target = target OP value`.
- `when { cond -> expr, ..., else -> expr }` — planned as **pure parser
  sugar**, desugaring straight to the existing nested `if`/`else if`/`else`
  AST. Zero changes needed in `infer.rs`/`codegen_c.rs` if done this way.
  Arms are newline-separated (same "skip newlines" loop as a block's
  statement list, since `when`'s body is inside `{}` where newlines are
  significant separators).
- No-parens `if`/`while` conditions — already true of the old grammar
  (parens were never required), so this is a no-op check, not new work.

**Explicitly deferred (not part of this rewrite pass), with reasons:**
- **Receiver-form methods** (`fun Circle.area()`, `fun mut Circle.grow()`)
  — `syntax&shii.md` itself places this at its own "M1" delta, not M0, and
  it drags in method-resolution-by-receiver-type plus the real
  implicit-shared-vs-`mut`-exclusive-borrow rule, which is borrow-checker
  territory (old M4). Functions stay free functions for now.
- **`pd` passdowns** — the doc's own §9 leaves the mutable-global/lifetime
  story as an open question with three unresolved options. Don't build
  against a decision the user hasn't made.
- **`impl` multi-file imports** — real loading was always M5 in
  `plan.md`; only parsing the `impl "path" [with caps(...)] [as alias]`
  syntax (so example source doesn't fail to parse) was being considered,
  with no semantic loading behind it yet.
- **String templates** (`"$var"`, `"${expr}"`) — doc's own §12 lists this
  under its "M2" delta, not M0.
- **`caps(...)` capability syntax** — depends on `impl` multi-file, above.

**Also noted, not yet handled:** `stdlib.md` (repo root) is a separate,
much larger companion doc about stdlib namespaces/`Bytes`/capability
wiring. Per explicit instruction, it is **out of scope right now** — do
not fold its decisions into the current rewrite. Revisit only when asked.

### Concrete file-level rewrite plan (drafted, not yet executed)

Everything downstream of the lexer needs touching, since the grammar
changed almost everywhere:
1. `lexer.rs` — remove `Fn`/`Let`/`Struct`/`Semi` handling; add
   `Fun`/`Val`/`Var`/`Datac`/`When`; add `Newline` token + paren-depth
   tracking; add compound-assign tokens; add the `ident!(` → `print!`
   fold. Keep `Mut` token defined (unused by the parser for now — still
   needed later for receiver/param mutability once that lands).
2. `ast.rs` — `Stmt::Let` → split conceptually into `val`/`var` (can stay
   one variant with a `mutable: bool` like today); drop `has_semi`;
   `StructDecl`/`FieldDecl` → `DatacDecl` with per-field `mutable: bool`;
   `FunctionDecl` gains a `.R: T` return-type span-carrying field (already
   has `return_type: Option<TypeExpr>`, so this may just be a parse-side
   change) and an expression-body variant; `Expr::Call` args become
   `Vec<CallArg>`; `Expr::StructLit` goes away (construction is just
   `Call` against a `datac` name, disambiguated at type-check time, same
   as calling a function); add compound-assign as a parse-time desugar
   (no new AST node needed).
3. `parser.rs` — large rewrite: statement separators, `datac`, `fun`
   signatures with `.R:`, call-arg list with optional `name =`, `when`
   desugaring, compound-assign desugaring. Drop `no_struct_lit` machinery
   (no longer needed).
4. `pretty.rs` — follow the AST changes (mechanical).
5. `types.rs` — `FnSig.params` becomes `Vec<(String, Ty)>` (needed for
   named-arg resolution); rename/generalize `StructDef` to whatever the
   `datac` type ends up called (can keep the name `StructDef` internally
   — it's an implementation type, not user-facing).
6. `infer.rs` — construction-via-call resolution (is this callee a
   function or a `datac` name?); named/positional arg matching (shared
   between plain calls and `datac` construction); per-field mutability
   check for field assignment (replaces the old root-binding-only rule);
   drop the `has_semi`-based tail-detection special case (already almost
   just "textually last", per above).
7. `codegen_c.rs` — mostly unaffected in spirit (still lowering the same
   kind of typed IR), but struct-literal codegen now needs to reorder
   named/positional args to the `datac`'s declared field order before
   emitting the designated-initializer C literal.
8. Examples/fixtures (`examples/*.x`, `xc/tests/fixtures/*.x`) and
   `tests/compile_and_run.rs` expected-output strings — rewritten to the
   new syntax; M3's planned `ownership_move.x`/`borrow_basic.x` fixtures
   (never created) still pending, now in new syntax.

### Recommended order when resuming

Lexer → AST → parser → pretty-printer (cheap, keep it compiling) →
`types.rs`/`infer.rs` → `codegen_c.rs` → rewrite examples/fixtures → get
`cargo test` green again → re-attempt M3 (moves) on top of the new syntax,
using the Copy-vs-non-Copy / per-scope move-state plan recorded above
(still valid, syntax-independent) — now with the bonus that `datac`'s
per-field mutability model was already forced to be built properly in
step 6 above, which move-checking will want anyway for partial-move
reasoning later.

# Rebrand: X → Kelp

The project currently calls itself **X**, ships a compiler called **xc**, uses the
`.x` file extension, a `x.toml` manifest, and an `alias.xconf` alias table. This
document records the target naming and everything that has to change to get
there.

## Brand register

| Thing | Old | New |
|---|---|---|
| Language | X | **Kelp** |
| File extension | `.x` | **`.kp`** |
| Build tool & package manager | *(none — `xc` doubles as it)* | **Coral** |
| LSP / server | *(none)* | **Lighthouse** |
| Commands | `xc build`, `xc run` | **`coral build`**, **`coral run`**, … |

## What exists today, and what does not

This is the important part. Only part of this rebrand is a rename; the rest is
naming a thing that has not been written.

| Brand | Status |
|---|---|
| **Kelp** | **Exists.** The language is real and implemented: 15,831 LOC of Rust across 19 files (16 in `xc/src/`, plus the 2,089-line e2e suite), 13 shipped stdlib modules, 120 fixtures, 39 test projects. This is a pure rename. |
| **`.kp`** | **Exists as `.x`.** 209 source files use the extension today. Pure rename. |
| **Coral** | **Does not exist.** Coral is `roadmap.md` **open item 72**, and its design sketch is `backlog.md`'s Coral section (the identifiers there are the `MK13-Sn-Mn` names it carried when it was a Mark). Nothing named Coral is written yet. |
| **Lighthouse** | **Does not exist, and is not in the plan.** There is no LSP, language server, editor, formatter, or tree-sitter grammar anywhere in `plan/`. Lighthouse is a new name for an unwritten component. This rebrand *introduces* it. |

Two consequences follow from that table, and neither is a rename:

1. **There is no Coral binary to rename.** Today `xc` is the compiler *and* the
   build tool — `basicstructure.md` states it outright: *"No separate build-tool
   layer; `xc` itself is the build tool."* So `coral build` / `coral run` cannot
   be shipped by this rebrand. Until Coral is scheduled, the working commands are the
   current ones under a new name.
2. **There is no Lighthouse code, and no plan document that describes it.** A
   rename cannot create it. This document fixes the name only.

## Open decisions

These are not settled by the naming above. Each needs an answer before or during
execution.

| # | Question | Options | Note |
|---|---|---|---|
| 1 | What is the **compiler binary** called? | `kelpc`, or fold it into `coral` | The roadmap keeps them distinct: *"Coral owns the manifest, the task graph, dependencies, caching and artifacts, and leaves compilation to `xc`"* (the Coral design, `backlog.md`). So the compiler needs its own name. `kelpc` is proposed throughout this document. |
| 2 | Does the **`coral` CLI replace `xc` now**? | (a) rename `xc` → `kelpc` now, add `coral` when Coral lands; (b) rename `xc` → `coral` now and grow Coral's real features later | (b) ships `coral build` immediately but makes `coral` a compiler today and a build tool later — a compatibility story the plan has not decided (the compiler-CLI contract in `backlog.md`'s Coral design exists precisely because today's CLI is not stable). (a) is proposed. |
| 3 | Manifest filename | `coral.toml`, `kelp.toml`, `Kelp.toml` | Roadmap says Coral owns the manifest, which argues for `coral.toml`. The manifest is currently `x.toml`, i.e. named after the *language*, which argues for `kelp.toml`. |
| 4 | Alias table filename | `alias.kpconf`, `alias.kelp`, `aliases.kp` | Currently `alias.xconf` — that is `x` + `conf`. The direct translation is `alias.kpconf`. |
| 5 | Repository name | rename the remote, or leave it | Remote is currently `…/Juniper/x.git`. Not a code change. |
| 6 | Stdlib module namespace | keep `std/…` | `impl "std/fs"` is unaffected by the language name. No change proposed. |

## Rename map

Every old token, where it lives, and how many occurrences there are. Counts are
measured, not estimated.

| Old | New | Files | Code refs | Doc refs |
|---|---|---|---|---|
| `.x` (extension) | `.kp` | 209 | ~231 | 150 |
| `x.toml` | `coral.toml` (decision 3) | 9 | 17 | 39 |
| `alias.xconf` | `alias.kpconf` (decision 4) | 2 | 36 | 13 |
| `xc` (binary/crate) | `kelpc` (decision 1) | 3 Cargo files + `main.rs` ×2 + `CARGO_BIN_EXE_xc` + `bench/*/run.sh` ×3 | ~9 | 94 |
| `x_` (generated-C symbol prefix) | `kelp_` | `codegen_c.rs`, `externs.rs` | **575** | — |
| `__X_HTTP_POOL__` (stdlib placeholder) | `__KELP_HTTP_POOL__` | `stdlib.rs` | 1 | 1 |
| `X` (language name, prose) | `Kelp` | `plan/*.md`, code comments, runtime banners | ~10 | 88 |

Generated-C prefix examples: `x_panic` (111), `x_Bytes` (34), `x_String` (28),
`x_fd` (25), `x_path` (19), `x_Bytes__…`, `x_String__from_str`, `x_List__String`,
`x_init_pds`.

### What must **not** be renamed

`x` is **not a keyword**. It is an ordinary identifier used as a variable name,
a field name, a named argument, and a loop counter throughout the fixtures:

```
val p = Point(x = 1, y = 2)
val z = p.x.y
```

154 occurrences of the bare identifier `x` live in `.kp` source (37 files), plus
thousands of ordinary `x_`-prefixed local variables in generated C (e.g. `x_s`,
`x_buf` — those are codegen locals, not the runtime ABI prefix, but they share
the `x_` spelling and must be told apart from it). The extension changes; the
*source contents* do not. A blind global substitution corrupts the language.

### The structural prerequisite

Only `alias.xconf` currently has named constants (`preprocess.rs`
`XCONF_MODULE` / `XCONF_FILE_NAME`). The `.x` extension, `x.toml`, and the
binary name are **39 hardcoded magic strings** scattered across `resolve.rs`,
`main.rs`, and `stdlib.rs`:

```
resolve.rs:529   self.src_dir.join(id).with_extension("x"),
resolve.rs:533   let path = self.src_dir.join(format!("{id}.x"));
resolve.rs:694   entry: "src/main.x".to_string(),
resolve.rs:954   if path.ends_with(".x") {
main.rs:75       "usage: xc build <file.x> [-o <out>]\n       xc run <file.x>"
```

Doing the rename before extracting constants replaces 39 magic strings with 39
different magic strings. Extract first, then rename through the constants, and
every future rebrand is a one-line change.

## Phased plan

| Phase | Work | Risk |
|---|---|---|
| **0. Constants** | Add `EXT`, `MANIFEST_NAME`, `BIN_NAME` (and reuse the existing `XCONF_*`) in `resolve.rs` / `main.rs`; route all 39 literals through them. | None. No behaviour change; the 2,089-line e2e suite must stay green. |
| **1. Compiler identity** | `xc` → `kelpc`: `Cargo.toml` (workspace member, `[package] name`, `[[bin]] name`), `main.rs` usage strings, `CARGO_BIN_EXE_xc` in `compile_and_run.rs`, `bench/*/run.sh` (`XC=`, `cargo build -p xc`, `target/release/xc`). | Low. |
| **2. Manifest & alias file** | `x.toml` → `coral.toml` (9 files), `alias.xconf` → `alias.kpconf` (2 files), plus the messages that name them. | Low. |
| **3. Extension** | 209 file renames `*.x` → `*.kp`, including `xc/tests/projects/*/src/**`, `xc/std/`, `xc/tests/fixtures/`, `examples/`, `bench/*/`. Update `include_str!` paths in `stdlib.rs` and the `entry` values inside the 9 manifests. | Low, but broad. Missed `include_str!` fails at compile time, not runtime. |
| **4. Generated-C ABI prefix** | `x_` → `kelp_` across 575 occurrences in `codegen_c.rs` and `externs.rs` string literals. `__X_HTTP_POOL__` → `__KELP_HTTP_POOL__`. | Low but tedious; two names sharing the `x_` spelling (runtime ABI vs. ordinary codegen locals) must be distinguished. |
| **5. Prose** | 396 KB of `plan/*.md`: 88 bare `X`, 150 `.x`, 39 `x.toml`, 94 `xc`, 13 `alias.xconf`. Titles (`The "X" language …`) and the docs that describe the extension and manifest. | Human review needed for ambiguous bare `X`. |
| **6. External** | Repository name, any URLs, and the future `coral` / `lighthouse` scaffolds. | Outside the code tree. |

### Post-rebrand state

- `kelpc build <file.kp>` and `kelpc run <file.kp>` work (decision 2a), then
  become `coral build` / `coral run` when Coral lands.
- `coral.toml` is the manifest, `alias.kpconf` the alias table, `.kp` the
  extension.
- Coral and Lighthouse remain named-and-unbuilt.

## Cost

| Phase | Estimate |
|---|---|
| 0. Constants | ~2 h |
| 1. Compiler identity | ~1 h |
| 2. Manifest & alias file | ~1 h |
| 3. Extension (209 files) | ~1 h |
| 4. Generated-C prefix (575) | ~2 h |
| 5. Prose (396 KB) | ~3–4 h |
| 6. External | ~1 h |
| **Total** | **~1.5 days** |

The docs are the largest single cost, not the code. The 2,089-line end-to-end
suite is the safety net: a broken extension, manifest, or symbol rename fails it
immediately, which is why every phase above except 5 is mechanical.

## Why now

- **No external surface exists.** There is no published crate, no install
  layout (`kelpc` embeds the stdlib with `include_str!` and is self-contained),
  no registry, no consumers. The cost of renaming rises the moment any of those
  exist.
- **The brand surface is still enumerable** — six token classes, all counted
  above.
- **The suite makes it safe.** 120 fixtures + 39 projects + ASAN coverage turn a
  rename into a green-or-red result.
- **Cost only grows.** Every new fixture, stdlib module, and doc line adds to
  phases 3–5.

Counterpoint, stated plainly: the codebase is *not* small (15.8k LOC, 396 KB of
docs, 209 source files), and Coral and Lighthouse are not renames — they are
names for unwritten software. This document makes the rename cheap and the
naming final; it does not build either tool.

## Executed

Done on branch `rebrand/x-to-kelp`. Every open decision was taken as proposed
above:

| # | Question | Taken |
|---|---|---|
| 1 | Compiler binary | `kelpc` |
| 2 | Does `coral` replace `xc` now? | (a) rename `xc` to `kelpc` now; `coral` arrives with Coral |
| 3 | Manifest filename | `coral.toml` |
| 4 | Alias table filename | `alias.kpconf` |
| 5 | Repository name | not renamed (not a code change) |
| 6 | Stdlib module namespace | unchanged (`std/…`) |

What each phase produced:

| Phase | Result |
|---|---|
| 0. Constants | `resolve::EXT`, `resolve::MANIFEST_NAME`, `main::BIN_NAME`; every literal routed through them, so a future rebrand changes values, not call sites. `preprocess::XCONF_*` became `preprocess::KPCONF_*`. |
| 1. Compiler identity | crate directory `xc/` became `kelpc/`; `[package]`/`[[bin]]` name, `CARGO_BIN_EXE_kelpc`, `cargo build -p kelpc`, `target/release/kelpc`, bench `KELPC=`. |
| 2. Manifest and alias file | `x.toml` to `coral.toml`, `alias.xconf` to `alias.kpconf`; test projects `alias_xconf`/`alias_xconf_cycle` to `alias_kpconf`/`alias_kpconf_cycle`. |
| 3. Extension | 209 files `.x` to `.kp`; `include_str!` paths, manifest `entry` values, fixtures, examples, bench. |
| 4. Generated-C ABI | `x_` to `kelp_` in `codegen_c.rs`, `externs.rs` and `types.rs` (plus comments in `infer.rs`, the e2e suite, and `std/io.kp`); `__X_HTTP_POOL__` to `__KELP_HTTP_POOL__`. |
| 5. Prose | `plan/*.md`, code comments and bench docs: `X` to `Kelp`, `.x` to `.kp`, `x.toml` to `coral.toml`, `xc` to `kelpc`, `alias.xconf` to `alias.kpconf`. Articles were corrected by hand where the word swap read wrong (`an Kelp` to `a Kelp`, `an coral.toml` to `a coral.toml`). |
| 6. External | repository name left alone; `coral` and `lighthouse` remain names without code. |

Names that were **not** touched, because they are ordinary identifiers: field
and variable spellings such as `p.x`, `q.x`, `origin.x`, `(*r).x` and `bag.xs`
in sources and docs; and the `box_`/`prefix_`/`index_` families in the compiler.

Verification: `cargo test` green after the rename — 65 unit tests plus 160
end-to-end tests, which include the ASAN and leak checks.

Unchanged from the analysis above: Coral and Lighthouse exist as names only.
`kelpc build` and `kelpc run` are the working commands until Coral lands;
`coral build` and `coral run` do not exist yet.

The file paths in this document (`xc/src/`, `bench/*/run.sh`, `xc/tests/…`)
describe the tree *before* the rename; that crate directory is `kelpc/` today.

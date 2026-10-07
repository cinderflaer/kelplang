# Narrowing and forwarding capabilities (MISSING-t9Qmz, MISSING-Iq2Tz)

*Design note, written in 26.10.3 ahead of the implementation. XL. Nothing here is built. It is
the "design note one release before" that `plan/roadmap.md` asks of every XL item, and it records
which half is a compile-time rule and which half needs a runtime value.*

## 1. The two gaps

**Forwarding (t9Qmz).** A grant is source text on an `impl` line, and a delegator may give a module
less than it holds, never more (`resolve.rs`). A library that performs a gated call on its caller's
behalf has to *write* the pattern it forwards, and the only pattern that fits every caller is a
wildcard (`caps(net = ["*"])`). The caller then cannot narrow it: the delegation rule refuses a
narrower grant than the one the library was written with.

**Narrowing at runtime (Iq2Tz).** Grants are static. A plugin or a worker cannot be handed a
narrower grant at run time, and a grant cannot be revoked, bounded in lifetime, or counted.

They are different mechanisms and ship separately, forwarding first.

## 2. Forwarding: `inherit`

A library says "whatever my importer holds for this domain" instead of naming a pattern:

```kelp
// lib.kp
impl "std/ws" caps(net = inherit, rand = [], time = [])
```

Rules:

1. `inherit` is a clause value like a pattern list. It is legal only for a domain the *library's
   own importer* holds (the delegation rule applies one hop up: `inherit` cannot exceed it).
2. The effective grant of the target is the library's own grant for that domain, evaluated per
   importer: the library holds whatever the importer that reached it holds, so a caller that wrote
   `impl "lib" caps(net = ["127.0.0.1:80"])` runs the library with exactly that.
3. **One library, two importers with different grants.** The generated capability check is per
   module today (`CapsGrant { module, domain, patterns }`). A module reached with two different
   effective grants cannot share one check. The decision: the module is **instantiated once per
   distinct effective grant**, the way a generic function is instantiated per type argument. The
   key is `(module, grant id)`; functions and statics of the module are duplicated per instance.
   The alternative (take the union) widens a narrow importer to what another importer holds, which
   is a privilege escalation, and is refused. The alternative (take the intersection) makes a
   library's behaviour depend on who else imports it, and is refused for the same reason it is in
   every capability system: authority must not be ambient.
4. A module that is reached by exactly one effective grant is not duplicated, so the common case
   costs nothing, and a project can ask `kelpc` which modules were instantiated more than once
   (`kelpc check --json` reports it) because duplication is a code-size cost.
5. `inherit` composes: a module that inherits and passes `inherit` to its own dependency forwards the
   same grant down the chain, each hop checked against the hop above.

What changes in the compiler: the `caps(...)` parser accepts `inherit` per domain; `resolve.rs`
computes effective grants top-down per path instead of per module and rejects `inherit` where the
importer holds nothing; the module graph gains instances; `infer.rs` qualifies a gated key with the
instance so the matcher's table is per instance. The matcher itself, the `Capability` domains and the
codegen of the check do not change.

Tests: the repro in MISSING-t9Qmz (a library forwarding for a caller that narrows to one port), two
importers with different grants of one library, `inherit` from an importer that holds nothing
(refused with the hop named), and `inherit` chains of depth three.

## 3. Runtime narrowing: capability values

The static rule cannot express "give this worker only this directory, decided at run time", so the
runtime half is a *value*.

1. **A capability value is a typed handle created from a module's grant**: `fs.cap()` yields an
   `fs.Cap` that holds the module's `fs` patterns (a value the module may pass on, never a global).
   It is not `Copy`; it is moved, or lent by borrow.
2. **Narrowing is a method that returns a smaller value**: `cap.narrow("/var/log/app/**")` is
   refused at run time (a typed error, not a panic) when the new pattern is not contained in the
   old one, using the matcher that already decides containment for delegation.
3. **Gated operations gain a `_with` form that takes the value**: `fs.read_text_with(cap, path)`.
   The ambient form stays and keeps checking the module's static grant; the `_with` form checks the
   value, so a function that receives a capability needs no `caps(...)` of its own for that domain
   (it does need the `impl` of the module, which is a compile-time statement of *intent*).
4. **Revocation and bounds** are fields of the value, not of the grant: `cap.revoke_after(n)` (a call
   budget) and `cap.revoker()` (a handle that revokes every value derived from this one). A revoked
   or exhausted value fails its checks with a typed error. The state is one shared cell per root
   value, so a derived value observes its ancestor's revocation.
5. **Threads.** A capability value crosses a thread boundary by `move` like any owned value; a
   revoker is `Send`. A scoped block cannot capture one by shared borrow past the call (the existing
   rule).

What this does not change: the static grant stays the ceiling. A value can only be narrower than the
module's grant, so the threat model's "no module holds more than its importer gave it" still holds,
and a freestanding image carries no runtime for it unless the project imports the module (the
domains are ordinary library code over the existing matcher).

Tests: narrowing inside and outside the pattern, a worker thread handed a narrowed value that tries
the parent directory, revocation observed by a derived value, a budget running out, and the same
operations through the static form unchanged.

## 4. Order of work

1. `inherit` (section 2), with the instance-per-grant resolver. This closes MISSING-t9Qmz.
2. Capability values for `fs` and `net` (section 3, items 1 to 3), then the other domains.
3. Revocation and budgets (item 4).

## 5. Open questions

- Whether the `_with` forms replace the ambient ones in the standard library over time (a single
  checked path) or stay a second surface. The conservative answer is to keep both until a release
  has used the value form in anger.
- Whether `inherit` should be allowed on a specific pattern ("inherit, but no more than X"). The
  intersection operator already exists for delegation, so it would be one more clause form, but it
  is not needed to close the repro and is left out.

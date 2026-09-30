# Which capability domain does a window belong to? (decision needed)

*Decided and implemented (option 2). `plan/user-ffi-gap-analysis.md` gap 6.*

A `[extern."<module id>"]` section must name one of the capability domains (`fs`, `net`, `process`, `env`,
`time`, `rand`), and an importer grants it with `impl "<module>" caps(<domain> = [...])`. A GTK
binding needs authority over the display server (a Wayland or X11 socket), which is none of them.

## Options

1. **Reuse `net`.** No compiler change. It is a lie: the grant would read "this module may open
   sockets", and a reviewer would not see a display connection in it.
2. **Add a `ui` domain.** One new name in `stdlib::DOMAINS`, and a matcher for its patterns. It is
   honest, and the grant line says what the module can do. The cost is the "no new domain names"
   rule the B-1 design pass set for its own scope; that scope is finished.
3. **Add a per-display domain (`display = ["wayland-0"]`).** The precise version. It needs the
   display name at the gated call, and GTK calls do not take one: the name comes from
   `$WAYLAND_DISPLAY`/`$DISPLAY` inside GTK. Checking it means reading the environment at run
   time, which is roadmap C-1 (runtime-derived authority) and not built.

## Recommendation

Option 2, with the pattern vocabulary reduced to what can be checked today: `ui = ["*"]` grants
the domain and no grant denies it, because no argument at a `gtk_*` call can be matched. State that
limit in the domain's documentation so nobody reads `ui = ["wayland-0"]` as enforced. Revisit with
option 3 when C-1 lands.

## Implemented

Option 2, with the pattern vocabulary the recommendation asked for: `ui` is the seventh entry in
`stdlib.rs::DOMAINS`, so an `[extern."<module>"] domain = "ui"` section is gated like any other,
and `examples/polyp-spike` now writes `domain = "ui"` and `caps(ui = ["*"])`.

Matching is what the recommendation said: `ui = ["*"]` grants the domain, no grant denies it at
resolution (`infer.rs::check_capability`, a compile error the way any ungranted gated name is), and
`resolve.rs::validate_cap_grant` refuses every other pattern list on an `impl` line or a `[caps]`
line with a message that patterns are not enforceable in this domain yet and only `["*"]` is
accepted. Fixtures `ffi_ui_domain`, `ffi_ui_pattern` and `ffi_ui_denied`; the domain list itself is
pinned in `stdlib.rs`'s unit tests. Revisit with option 3 when C-1 lands, and the `["*"]`-only rule
is the line to relax then.

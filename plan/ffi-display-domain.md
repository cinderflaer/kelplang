# Which capability domain does a window belong to? (decision needed)

*A decision note, not an implementation. `plan/user-ffi-gap-analysis.md` gap 6.*

A `[extern."<module id>"]` section must name one of six domains (`fs`, `net`, `process`, `env`,
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

Until this is decided, prototypes should use a stand-in domain and say so in a comment.

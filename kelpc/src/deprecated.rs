//! Outdated syntax: what the compiler still accepts, and means to remove.
//!
//! A language change that replaces or deletes a spelling does not have to break
//! code the day it lands, and it should not: the old spelling keeps compiling
//! until its readers have had a cycle to move off it. This module is the
//! register of that state. An entry is *data* (an id, what is outdated, what to
//! write instead), and the pass that reads the construct raises the finding, so
//! the wording and the deadline of every deprecation live in one place.
//!
//! What an entry buys, and what it does not:
//!
//! - The old spelling **compiles**, exit status and all. `kelpc build` and
//!   `kelpc run` print the finding as an `outdated:` warning, and `kelpc fmt`
//!   copies the text exactly, because a formatter that never rewrites a token
//!   cannot rewrite this one either.
//! - The warning **names the removal**, and the deadline is one shared
//!   sentence ([`REMOVAL`]) rather than a phrase per entry: an entry is removed
//!   on the next development cycle, and nothing here can quietly promise a
//!   different one.
//! - `--deny-outdated` makes those findings **errors**: the same messages, and
//!   the build stops before anything is generated. That is the migration gate.
//!   It is off by default, because the compiler's job is to build a program
//!   that was written for the version it has, not to refuse the version before
//!   it.
//! - The compiler's **own** sources never carry an outdated spelling: an
//!   outdated construct in the shipped stdlib would warn every user of that
//!   module about code they cannot edit. The test at the bottom is what keeps
//!   that true.
//! - An entry here is **scheduled work with a deadline**, not an open defect.
//!   the wiki's open issues page is where what is wrong lives; a spelling that still works is
//!   not wrong, it is on its way out.
//!
//! Retiring an entry, in the order that keeps the tree consistent: delete the
//! spelling from the lexer or parser (and from the shipped stdlib, and from the
//! fixtures that were deliberately left on it), delete the entry here, and let
//! `kelpc/tests/outdated.rs` and the shipped-source test below say what is left.

use crate::diag::Diagnostic;
use crate::lexer::{Span, Token, TokenKind};

/// The deadline every entry carries, as one sentence so it cannot drift.
const REMOVAL: &str = "will be removed in the next development cycle";

/// One thing the compiler still accepts and will not accept later.
pub struct Outdated {
    /// Stable, kebab-case id. A test, a commit message and the plan name an
    /// entry by this, so its wording can change without the references to it
    /// changing.
    pub id: &'static str,
    /// What is outdated, in the words a reader searches the source for.
    pub what: &'static str,
    /// What to write instead, and why that means the same thing.
    pub instead: &'static str,
}

/// Every outdated construct, in the order they were scheduled.
///
/// Every outdated construct, in the order they were scheduled.
///
/// The `impl` clauses that were here before were removed in the cycle after
/// `26.09.0`, on the deadline each carried. The current entries are `Map.new()`
/// (issue #302, `Map.empty()` is its spelling), the two numeric names (issue
/// #279): `to_u8` and `try_to_u8` still build, and the everyday `to_byte` and
/// `try_to_byte` are their spellings, and the six text names (issue #262):
/// `fmt.int`, `fmt.uint`, `fmt.float`, `str.from_u64`, `str.from_i64` and
/// `str.from_f64` still build, and `toText` on the number is their spelling.
/// The `try_` twins of the fallible std names (issue #264): `fs.try_read` and the
/// rest of the pairs still build, and the plain name returns the same `Result`.
///
/// The register stays the mechanism for the next deprecation: add an entry here,
/// raise its finding where the spelling is read (`scan_tokens` for a method
/// name), and remove the spelling on the next development cycle. An entry
/// survives only while the spelling still compiles, and the removal is the
/// parser's or the checker's, not this module's.
pub const REGISTER: &[Outdated] = &[
    Outdated {
        id: "map-new",
        what: "`Map.new()`",
        instead: "write `Map.empty()`, which starts the same empty map",
    },
    Outdated {
        id: "numeric-to-u8",
        what: "`.to_u8()`",
        instead: "write `.to_byte()`, which converts to the same `u8`",
    },
    Outdated {
        id: "numeric-try-to-u8",
        what: "`.try_to_u8()`",
        instead: "write `.try_to_byte()`, which hands back the same `Option<u8>`",
    },
    Outdated {
        id: "fmt-int",
        what: "`fmt.int(`",
        instead: "write `n.toText()`, which gives the same decimal text of an integer",
    },
    Outdated {
        id: "fmt-uint",
        what: "`fmt.uint(`",
        instead: "write `n.toText()`, which gives the same decimal text of an unsigned integer",
    },
    Outdated {
        id: "fmt-float",
        what: "`fmt.float(`",
        instead: "write `x.toText(decimals)`, which gives the same text with `decimals` digits after the point (`x.toText(2)` for the default of two)",
    },
    Outdated {
        id: "str-from-u64",
        what: "`str.from_u64(`",
        instead: "write `n.toText()`, which gives the same decimal text of an unsigned integer",
    },
    Outdated {
        id: "str-from-i64",
        what: "`str.from_i64(`",
        instead: "write `n.toText()`, which gives the same decimal text of an integer",
    },
    Outdated {
        id: "str-from-f64",
        what: "`str.from_f64(`",
        instead: "write `x.toText(decimals)`, which gives the same text with `decimals` digits after the point",
    },    Outdated {
        id: "try-fs-read",
        what: "`fs.try_read(`",
        instead: "write `fs.read(`, which returns the same `Result`; a panic is `result.expect(fs.read(...), \"...\")`",
    },
    Outdated {
        id: "try-fs-read-text",
        what: "`fs.try_read_text(`",
        instead: "write `fs.read_text(`, which returns the same `Result`; a panic is `result.expect(fs.read_text(...), \"...\")`",
    },
    Outdated {
        id: "try-fs-write",
        what: "`fs.try_write(`",
        instead: "write `fs.write(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-write-text",
        what: "`fs.try_write_text(`",
        instead: "write `fs.write_text(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-remove",
        what: "`fs.try_remove(`",
        instead: "write `fs.remove(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-rename",
        what: "`fs.try_rename(`",
        instead: "write `fs.rename(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-append",
        what: "`fs.try_append(`",
        instead: "write `fs.append(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-list",
        what: "`fs.try_list(`",
        instead: "write `fs.list(`, which returns the same `Result`; a panic is `result.expect(fs.list(...), \"...\")`",
    },
    Outdated {
        id: "try-fs-mkdir-all",
        what: "`fs.try_mkdir_all(`",
        instead: "write `fs.mkdir_all(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-remove-all",
        what: "`fs.try_remove_all(`",
        instead: "write `fs.remove_all(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-symlink",
        what: "`fs.try_symlink(`",
        instead: "write `fs.symlink(`, which returns `Result<unit, io.IoErr>`: match `Err(e)` where this matched `Some(e)`, or write `.expect(\"...\")` for a panic",
    },
    Outdated {
        id: "try-fs-open",
        what: "`fs.try_open(`",
        instead: "write `fs.open(`, which returns the same `Result`; a panic is `result.expect(fs.open(...), \"...\")`",
    },
    Outdated {
        id: "try-process-run",
        what: "`process.try_run(`",
        instead: "write `process.run(`, which returns the same `Result`; a panic is `result.expect(process.run(...), \"...\")`",
    },
    Outdated {
        id: "try-process-run-in",
        what: "`process.try_run_in(`",
        instead: "write `process.run_in(`, which returns the same `Result`; a panic is `result.expect(process.run_in(...), \"...\")`",
    },
    Outdated {
        id: "try-net-connect",
        what: "`net.try_connect(`",
        instead: "write `net.connect(`, which returns the same `Result`; a panic is `result.expect(net.connect(...), \"...\")`",
    },
    Outdated {
        id: "try-net-send-all",
        what: "`net.try_send_all(`",
        instead: "write `net.send_all(`, which returns the same `Result`; a panic is `result.expect(net.send_all(...), \"...\")`",
    },
    Outdated {
        id: "try-net-recv",
        what: "`net.try_recv(`",
        instead: "write `net.recv(`, which returns the same `Result`; a panic is `result.expect(net.recv(...), \"...\")`",
    },
    Outdated {
        id: "try-net-recv-into",
        what: "`net.try_recv_into(`",
        instead: "write `net.recv_into(`, which returns the same `Result`; a panic is `result.expect(net.recv_into(...), \"...\")`",
    },
    Outdated {
        id: "try-net-accept",
        what: "`net.try_accept(`",
        instead: "write `net.accept(`, which returns the same `Result`; a panic is `result.expect(net.accept(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-handshake",
        what: "`tls.try_handshake(`",
        instead: "write `tls.handshake(`, which returns the same `Result`; a panic is `result.expect(tls.handshake(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-handshake-with",
        what: "`tls.try_handshake_with(`",
        instead: "write `tls.handshake_with(`, which returns the same `Result`; a panic is `result.expect(tls.handshake_with(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-handshake-timeout",
        what: "`tls.try_handshake_timeout(`",
        instead: "write `tls.handshake_timeout(`, which returns the same `Result`; a panic is `result.expect(tls.handshake_timeout(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-write",
        what: "`tls.try_write(`",
        instead: "write `tls.write(`, which returns the same `Result`; a panic is `result.expect(tls.write(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-write-all",
        what: "`tls.try_write_all(`",
        instead: "write `tls.write_all(`, which returns the same `Result`; a panic is `result.expect(tls.write_all(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-read",
        what: "`tls.try_read(`",
        instead: "write `tls.read(`, which returns the same `Result`; a panic is `result.expect(tls.read(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-read-into",
        what: "`tls.try_read_into(`",
        instead: "write `tls.read_into(`, which returns the same `Result`; a panic is `result.expect(tls.read_into(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-shutdown",
        what: "`tls.try_shutdown(`",
        instead: "write `tls.shutdown(`, which returns the same `Result`; a panic is `result.expect(tls.shutdown(...), \"...\")`",
    },
    Outdated {
        id: "try-tls-handshake-opts",
        what: "`tls.try_handshake_opts(`",
        instead: "write `tls.handshake_opts(`, which returns the same `Result`; a panic is `result.expect(tls.handshake_opts(...), \"...\")`",
    },
    Outdated {
        id: "try-ws-connect",
        what: "`ws.try_connect(`",
        instead: "write `ws.connect(`, which returns the same `Result`; a panic is `result.expect(ws.connect(...), \"...\")`",
    },
    Outdated {
        id: "try-ws-connect-url",
        what: "`ws.try_connect_url(`",
        instead: "write `ws.connect_url(`, which returns the same `Result`; a panic is `result.expect(ws.connect_url(...), \"...\")`",
    },
    Outdated {
        id: "try-ws-connect-scheme",
        what: "`ws.try_connect_scheme(`",
        instead: "write `ws.connect_scheme(`, which returns the same `Result`; a panic is `result.expect(ws.connect_scheme(...), \"...\")`",
    },
    Outdated {
        id: "try-http-get",
        what: "`http.try_get(`",
        instead: "write `http.get(`, which returns the same `Result`; a panic is `result.expect(http.get(...), \"...\")`",
    },
    Outdated {
        id: "try-http-get-tls",
        what: "`http.try_get_tls(`",
        instead: "write `http.get_tls(`, which returns the same `Result`; a panic is `result.expect(http.get_tls(...), \"...\")`",
    },
    Outdated {
        id: "try-net-shutdown-write",
        what: "`net.try_shutdown_write(`",
        instead: "write `net.shutdown_write(`, which returns `Result<unit, NetErr>`: `.is_ok()` where this returned a `bool`",
    },
];

/// The register entry with this id.
///
/// Nothing in the compiler calls this while the register is empty, and that is
/// the point: it is the read path the next entry arrives through, kept with
/// `finding` so adding an entry is data rather than new code. The tests below
/// are what keep the two exercised.
pub fn find(id: &str) -> &'static Outdated {
    REGISTER
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("'{id}' is not in the register of outdated syntax"))
}

/// The finding for an entry, at the span that names the construct.
fn finding_for(entry: &Outdated, span: Span, module: &str) -> Diagnostic {
    Diagnostic::new(
        format!("{} is outdated and {REMOVAL}: {}", entry.what, entry.instead),
        span,
        module,
    )
}

/// The finding for the registered entry `id`, at `span` in `module`.
pub fn finding(id: &str, span: Span, module: &str) -> Diagnostic {
    finding_for(find(id), span, module)
}

/// The register id of a retired `try_` twin, by its owner module and name
/// (issue #264). `None` for every other spelling.
fn twin_id(owner: &str, name: &str) -> Option<&'static str> {
    Some(match (owner, name) {
        ("fs", "try_read") => "try-fs-read",
        ("fs", "try_read_text") => "try-fs-read-text",
        ("fs", "try_write") => "try-fs-write",
        ("fs", "try_write_text") => "try-fs-write-text",
        ("fs", "try_remove") => "try-fs-remove",
        ("fs", "try_rename") => "try-fs-rename",
        ("fs", "try_append") => "try-fs-append",
        ("fs", "try_list") => "try-fs-list",
        ("fs", "try_mkdir_all") => "try-fs-mkdir-all",
        ("fs", "try_remove_all") => "try-fs-remove-all",
        ("fs", "try_symlink") => "try-fs-symlink",
        ("fs", "try_open") => "try-fs-open",
        ("process", "try_run") => "try-process-run",
        ("process", "try_run_in") => "try-process-run-in",
        ("net", "try_connect") => "try-net-connect",
        ("net", "try_send_all") => "try-net-send-all",
        ("net", "try_recv") => "try-net-recv",
        ("net", "try_recv_into") => "try-net-recv-into",
        ("net", "try_accept") => "try-net-accept",
        ("tls", "try_handshake") => "try-tls-handshake",
        ("tls", "try_handshake_with") => "try-tls-handshake-with",
        ("tls", "try_handshake_timeout") => "try-tls-handshake-timeout",
        ("tls", "try_write") => "try-tls-write",
        ("tls", "try_write_all") => "try-tls-write-all",
        ("tls", "try_read") => "try-tls-read",
        ("tls", "try_read_into") => "try-tls-read-into",
        ("tls", "try_shutdown") => "try-tls-shutdown",
        ("tls", "try_handshake_opts") => "try-tls-handshake-opts",
        ("ws", "try_connect") => "try-ws-connect",
        ("ws", "try_connect_url") => "try-ws-connect-url",
        ("ws", "try_connect_scheme") => "try-ws-connect-scheme",
        ("http", "try_get") => "try-http-get",
        ("http", "try_get_tls") => "try-http-get-tls",
        ("net", "try_shutdown_write") => "try-net-shutdown-write",
        _ => return None,
    })
}

/// The outdated names a token stream uses: each `Map.new(` call, one finding per
/// call at the `new`, and each `.to_u8(` and `.try_to_u8(` call, one finding per
/// call, at the name. It reads the tokens
/// before parsing, so the parser never needs to know the register, and a
/// name that is not a method call (`to_u8` on its own, a `fun to_u8`) is left
/// alone. A user method of the same name is flagged too: the finding matches
/// the spelling, not the declaration.
pub fn scan_tokens(tokens: &[Token], module: &str) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    // `Map.new(`: the owner is the `Map` name, so no other type's `.new(` is read.
    for window in tokens.windows(4) {
        let [owner, dot, name, open] = window else { continue };
        if !matches!(&owner.kind, TokenKind::Ident(text) if text == "Map")
            || !matches!(dot.kind, TokenKind::Dot)
            || !matches!(open.kind, TokenKind::LParen)
        {
            continue;
        }
        if matches!(&name.kind, TokenKind::Ident(text) if text == "new") {
            found.push(finding("map-new", name.span, module));
        }
    }
    // `fmt.int(`, `fmt.uint(`, `fmt.float(`, `str.from_u64(`, `str.from_i64(` and
    // `str.from_f64(`: the two owners are the module names, so a call through
    // another name (a local, a user module) is left alone (issue #262).
    for window in tokens.windows(4) {
        let [owner_tok, dot, name_tok, open] = window else { continue };
        if !matches!(dot.kind, TokenKind::Dot) || !matches!(open.kind, TokenKind::LParen) {
            continue;
        }
        let (TokenKind::Ident(owner), TokenKind::Ident(name)) = (&owner_tok.kind, &name_tok.kind) else {
            continue;
        };
        let id = match (owner.as_str(), name.as_str()) {
        ("fmt", "int") => "fmt-int",
        ("fmt", "uint") => "fmt-uint",
        ("fmt", "float") => "fmt-float",
        ("str", "from_u64") => "str-from-u64",
        ("str", "from_i64") => "str-from-i64",
        ("str", "from_f64") => "str-from-f64",
            _ => continue,
        };
        found.push(finding(id, name_tok.span, module));
    }
    // The `try_` twins of the fallible std names (issue #264): `owner.try_x(`
    // for each module that owns one. The finding names the plain name to write,
    // and the spelling still builds for one cycle.
    for window in tokens.windows(4) {
        let [owner_tok, dot, name_tok, open] = window else { continue };
        if !matches!(dot.kind, TokenKind::Dot) || !matches!(open.kind, TokenKind::LParen) {
            continue;
        }
        let (TokenKind::Ident(owner), TokenKind::Ident(name)) = (&owner_tok.kind, &name_tok.kind) else {
            continue;
        };
        if let Some(id) = twin_id(owner, name) {
            found.push(finding(id, name_tok.span, module));
        }
    }
    for window in tokens.windows(3) {
        let [dot, name, open] = window else { continue };
        if !matches!(dot.kind, TokenKind::Dot) || !matches!(open.kind, TokenKind::LParen) {
            continue;
        }
        let TokenKind::Ident(text) = &name.kind else { continue };
        let id = match text.as_str() {
            "to_u8" => "numeric-to-u8",
            "try_to_u8" => "numeric-try-to-u8",
            _ => continue,
        };
        found.push(finding(id, name.span, module));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    /// An entry in the register's own shape, built here rather than registered:
    /// with the register empty there is no id to look up, and a test must not
    /// add a permanent deprecation to get one. What it pins is the wording an
    /// entry buys.
    fn sample_entry() -> Outdated {
        Outdated {
            id: "sample-entry",
            what: "`impl \"...\" with caps(...)`",
            instead: "drop the `with`: the grant is `impl \"std/fs\" caps(fs = [\"/var/log/**\"])`",
        }
    }

    #[test]
    fn every_id_is_unique() {
        let mut seen = Vec::new();
        for entry in REGISTER {
            assert!(
                !seen.contains(&entry.id),
                "two register entries share the id '{}'",
                entry.id
            );
            seen.push(entry.id);
        }
    }

    #[test]
    fn every_finding_names_the_removal_and_the_fix() {
        // The register is empty, so the sample is what keeps the wording under
        // test today; the loop over the register is what the next entry gets
        // for free, and a sample that passed while a real entry did not would
        // be a test of nothing.
        let sample = sample_entry();
        for entry in REGISTER.iter().chain(std::iter::once(&sample)) {
            let d = finding_for(entry, Span::new(0, 0), "m");
            assert!(d.message.contains("outdated"), "{}", d.message);
            assert!(
                d.message.contains("will be removed in the next development cycle"),
                "{}",
                d.message
            );
            assert!(
                d.message.contains(entry.instead),
                "the fix of '{}' is not in its own finding: {}",
                entry.id,
                d.message
            );
        }
    }

    #[test]
    fn an_unknown_id_is_a_compiler_bug() {
        // The only way to reach this is to raise a deprecation that was never
        // registered, which is a mistake to make loudly rather than to print.
        let caught = std::panic::catch_unwind(|| find("no-such-entry"));
        assert!(caught.is_err());
    }

    #[test]
    fn only_the_map_new_call_is_flagged_among_the_constructors() {
        // `Map.new(` is the one spelling flagged. The other constructors, a
        // `.new(` on another type, `Map.with_capacity(`, `Map.empty(` and a
        // bare `Map.new` with no call, are left alone (issue #302).
        let text = "var a = Map.new()\nvar b = Map.with_capacity(4)\nvar c = Map.empty()\n\
                    var d = Atomic.new(0)\nx.new(1)\nvar e = Map.new\n";
        let tokens = Lexer::new(text).tokenize().expect("the sample lexes");
        let found = scan_tokens(&tokens, "m");
        assert_eq!(found.len(), 1, "{:?}", found.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(found[0].message.contains("`Map.new()`"), "{}", found[0].message);
        assert!(found[0].message.contains("write `Map.empty()`"), "{}", found[0].message);
    }

    #[test]
    fn the_text_spellings_are_flagged_by_their_owner_module() {
        // The six retired names are flagged where the owner is `fmt` or `str`
        // (issue #262). A call through another owner, a user function named
        // `int`, and the `toText` replacement are left alone.
        let text = "var a = fmt.int(1)\nvar b = fmt.uint(2)\nvar c = fmt.float(3.0)\n\
                    var d = str.from_u64(4)\nvar e = str.from_i64(5)\nvar f = str.from_f64(6.0, 2)\n\
                    var g = other.int(7)\nvar h = n.toText()\nvar i = fmt.int\nvar j = str.from_bytes(b)\n";
        let tokens = Lexer::new(text).tokenize().expect("the sample lexes");
        let found = scan_tokens(&tokens, "m");
        let ids: Vec<&str> = found
            .iter()
            .map(|d| d.message.split(" is outdated").next().unwrap_or(""))
            .collect();
        assert_eq!(found.len(), 6, "{ids:?}");
        for spelling in ["`fmt.int(`", "`fmt.uint(`", "`fmt.float(`", "`str.from_u64(`", "`str.from_i64(`", "`str.from_f64(`"] {
            assert!(ids.contains(&spelling), "{spelling} not flagged: {ids:?}");
        }
    }

    #[test]
    fn the_try_twins_are_flagged_by_their_owner_module() {
        // The `try_` twins of the fallible std names are flagged where the owner
        // is the module that owns them (issue #264), and the finding names the
        // plain name the caller writes instead. A plain name, a `try_` name on
        // another owner and an unknown `try_` name are left alone.
        let text = "val a = fs.try_read(p)\nval b = fs.try_write(p, d)\nval c = process.try_run(a, b)\n\
                    val d2 = process.try_run_in(d, a, b)\nval e = net.try_connect(h, 1)\n\
                    val f = tls.try_write(t, d)\nval g = ws.try_connect(h, 1, p)\nval h2 = http.try_get(u)\n\
                    val i = fs.read(p)\nval j = fs.try_no_such(x)\nval k = local.try_read(x)\n";
        let tokens = Lexer::new(text).tokenize().expect("the sample lexes");
        let found = scan_tokens(&tokens, "m");
        let messages: Vec<&str> = found.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(found.len(), 8, "{messages:?}");
        for (spelling, plain) in [
            ("`fs.try_read(`", "`fs.read(`"),
            ("`fs.try_write(`", "`fs.write(`"),
            ("`process.try_run(`", "`process.run(`"),
            ("`process.try_run_in(`", "`process.run_in(`"),
            ("`net.try_connect(`", "`net.connect(`"),
            ("`tls.try_write(`", "`tls.write(`"),
            ("`ws.try_connect(`", "`ws.connect(`"),
            ("`http.try_get(`", "`http.get(`"),
        ] {
            let m = messages
                .iter()
                .find(|m| m.starts_with(spelling))
                .unwrap_or_else(|| panic!("{spelling} not flagged: {messages:?}"));
            assert!(m.contains(plain), "the finding for {spelling} does not name {plain}: {m}");
        }
    }

    #[test]
    fn the_shipped_sources_carry_no_outdated_spelling() {
        // A deprecation in the stdlib is the worst kind: every program that
        // imports that module would be warned about a file it cannot edit.
        //
        // Two things keep the stdlib clean: no shipped module writes a spelling
        // an entry owns (the scan below), and every shipped module parses (the
        // walk, which is where a refused spelling fails loudly, naming its fix).
        // The next entry adds its own spelling to the list.
        const RETIRED: &[&str] = &[
            "with caps(", ").R:", ").R ", ".to_u8(", ".try_to_u8(", "Map.new(", "fmt.int(", "fmt.uint(",
            "fmt.float(", "str.from_u64(", "str.from_i64(", "str.from_f64(", "fs.try_", "process.try_",
            "net.try_", "tls.try_", "ws.try_", "http.try_",
        ];
        let mut ids: Vec<&str> = crate::stdlib::shipped().to_vec();
        ids.push(crate::stdlib::RT_MODULE);
        for id in ids {
            let text = crate::stdlib::source(id).expect("a shipped module has source");
            for spelling in RETIRED {
                assert!(
                    !text.contains(spelling),
                    "the shipped module '{id}' writes `{spelling}...)`, which is outdated \
                     syntax: a program that imports it would be warned about a file it cannot edit"
                );
            }
            let tokens = Lexer::new(&text)
                .tokenize()
                .unwrap_or_else(|e| panic!("'{id}' does not lex: {}", e.message));
            if let Err(e) = Parser::new(tokens).parse_program() {
                panic!("'{id}' does not parse: {}", e.message);
            }
        }
    }
}

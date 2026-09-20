//! The differential corpus for the resolver's configuration parsing (MK7 §7
//! item 6, §8).
//!
//! §8's contract is "build each runtime piece in Kelp first, keep the C as the
//! reference, and prove it with a differential corpus". This is that, for
//! `/etc/hosts` and `/etc/resolv.conf`, the two files a stub resolver reads
//! before it sends anything. §7 names the resolver as the largest single
//! non-codegen item and the one most likely to be deferred; its *transport* half
//! needs a socket, and its configuration half needs none, so this is the half
//! that can be held down now.
//!
//! The two implementations:
//!
//!   * `kelpc/runtime/resolver_conf_ref.c`: the C reference. Nothing includes
//!     it but this test, which compiles it directly.
//!   * `kelpc/runtime/resolver_conf.kp`: the same parser in Kelp, compiled by
//!     the C backend. It is not a std module and is not shipped yet.
//!
//! Both are fed the same corpus and each prints one line per case; the test
//! compares the two line streams exactly (same count, same text, same order).
//! Every case contributes exactly one line, so a mismatch is the case at that
//! index.
//!
//! # What agreement here does and does not prove
//!
//! **Both implementations were written from the same rule choices, by the same
//! author, in the same session.** The corpus therefore proves that the Kelp port
//! and the C reference *agree*; it does not prove that either is right about
//! `/etc/hosts` or `/etc/resolv.conf`. Some of the rules below are glibc's
//! documented behaviour (`MAXNS` 3, `MAXDNSRCH` 6, the defaults, last-`search`-
//! wins) and some are choices this lane made, and the C reference follows the
//! choices too. The pinned hand-written lines are the only absolute check, and
//! they encode the same choices, so a rule this lane got wrong is a rule the
//! corpus agrees with. The choices worth a second opinion are named as
//! *choice* below.
//!
//! Measured on this corpus: of the 2612 cases only 71 carry an expectation at
//! all, 70 of them an absolute line and 1 only the `ok ` prefix, and every one
//! of those is checked against the C reference rather than an external
//! authority. The other 2541 cases assert only that the two implementations
//! agree. Where the sibling `dns_parse` corpus keeps four recorded responses as
//! the only externally sourced input in either corpus, this one has none: every
//! case is hand-written here or a mutation of one.
//!
//! # The covered contract
//!
//! Rendering. `/etc/hosts` is
//! `ok hosts` followed by one ` e=<addr>|<name>,<name>` group per file entry in
//! file order; `|` and `,` are the separators because neither can appear in an
//! address or a hostname and an IPv6 address is full of `:`. `/etc/resolv.conf`
//! is `ok ns=<list> nsdrop=<n> search=<list> searchdrop=<n> ndots=<n>
//! timeout=<n> attempts=<n> rotate=<true|false> unknown=<n>`, where `<list>` is
//! `,`-joined or `-`. A refusal prints `reject <reason>` and nothing else.
//!
//! Lines and comments. A line is everything up to `\n`, with a trailing `\r`
//! dropped (a CRLF file parses like an LF one). `#` starts a comment anywhere on
//! a line in both files, and `;` does too in `/etc/resolv.conf` (*choice*:
//! hosts(5) documents `#`, and BSD's resolver accepts both). A blank line and a
//! comment-only line are skipped. A bad line rejects the **whole file** with the
//! class of the first bad line, rather than being skipped (*choice*: glibc
//! ignores most of what it cannot read; the reason here is that a config file a
//! resolver partly ignores is a config file whose behaviour nobody can predict).
//! The reason does not name the line number.
//!
//! Addresses, one parser for both files.
//!
//! * IPv4 is four decimal octets of 1 to 3 digits, each at most 255, with no
//!   leading zero on a multi-digit octet (*choice*: `inet_aton` reads `010` as
//!   octal, so `010.0.0.1` and `10.0.0.1` would be the same address written two
//!   ways; this parser refuses the first). `1.2.3`, `1.2.3.4.5` and `1.2.3.4x`
//!   are refusals.
//! * IPv6 is 1 to 4 hex digits per group, `::` at most once, a single `:` with a
//!   group on both sides, and exactly eight groups after the gap is expanded. A
//!   zone suffix (`fe80::1%eth0`) and an embedded IPv4 tail (`::ffff:1.2.3.4`)
//!   are refusals (*choice*: neither is a bare address this resolver would
//!   compare against a peer's).
//! * The canonical spelling is lowercase, without leading zeros, with `::` for
//!   the longest run of two or more zero groups (the leftmost on a tie), which
//!   is RFC 5952.
//!
//! Hostnames. RFC 1123's preferred syntax and no more: 1 to 63 bytes per label
//! of ASCII letters, digits and hyphen, a hyphen not first or last in a label,
//! at least one label, at most 255 bytes in all, no trailing dot (*choice*: a
//! trailing dot is DNS's spelling for "absolute", and v1 has no search suffix to
//! make that distinct, so `foo.` is refused rather than merged with `foo`). An
//! underscore, a leading or trailing hyphen, an empty label (`a..b`) and every
//! non-ASCII byte are refusals, because a name with two spellings is a lookup
//! with two answers. Names are lowercased for the lookup (*choice*).
//!
//! `/etc/hosts` (hosts(5)): `address name [alias...]`, fields separated by
//! spaces or tabs, a trailing comment allowed. An address with no name is
//! `hosts-no-name`, an address that is not one is `hosts-bad-address`, and a
//! name that is not a hostname is `hosts-bad-name`; the address is validated
//! first, so a line whose address is also malformed reports the address.
//! Duplicate names are kept, not merged, and the entries stay in file order
//! (*choice*: which of two addresses for one name wins is the lookup policy's
//! decision, not the parser's; glibc's `hosts` lookup returns both).
//!
//! `/etc/resolv.conf` (resolv.conf(5), glibc's `res_init`). `nameserver` takes
//! exactly one address: none or more than one is `resolv-missing-argument` or
//! `resolv-extra-argument`, and a name that is not a numeric address is
//! `resolv-bad-address` (*choice*: glibc skips a non-numeric one rather than
//! refusing the file). At most `MAXNS` 3 nameservers are kept, in file order,
//! and the rest are counted in `nsdrop`. `domain` and `search` are the same
//! field, and the last instance of either wins, which is what resolv.conf(5)
//! says and what glibc does; `domain X` is a one-element search list, and a
//! search list is truncated at `MAXDNSRCH` 6 with the rest counted in
//! `searchdrop`. A search name that is not a hostname, and `domain .` (*choice*),
//! are `resolv-bad-search`. `options` sets `ndots` (0 to 15, default 1 *choice*
//! to keep glibc's default), `timeout` (1 to 30, default 5), `attempts` (1 to 5,
//! default 2) and `rotate`; a known option whose value is malformed or out of
//! range is `resolv-bad-option` (*choice*: glibc ignores it), while an option
//! that is not one of the four is **skipped** and counted in `unknown`
//! (*choice*, the one the task names as a coin flip: glibc ignores it too, but a
//! count is reported so the corpus can see it was skipped and not refused). The
//! options of several `options` lines accumulate, and a repeated option's last
//! value wins. A line whose first word is not one of the four directives is
//! `resolv-unknown-keyword`, matched case-sensitively.
//!
//! Every rejection class above is exercised by a hand-written case, and every
//! one of those has a case that would have accepted the same input with one
//! detail changed. The test asserts that both verdicts occur, per format, and
//! that every reason in `REASONS` is exercised.
//!
//! # Not covered, and why
//!
//! * The query transport: UDP, TCP, retries, timeouts and TCP fallback. Every
//!   one of them needs a socket, and this lane has none.
//! * Reading the files: no `open`, no `read`, no path, no default location, no
//!   `/etc/hosts` fallback when `resolv.conf` names an unreachable server.
//! * The resolver's own state: caching, negative caching, TTL handling, and
//!   which answer wins when a name has several.
//! * Search-domain application: `/etc/resolv.conf`'s search list is *parsed* and
//!   `ndots` is *read*, and neither is ever used to build a query, so the rules
//!   for trying a short name against each suffix are not tested here.
//! * The `sortlist` and `aliases` directives, `RES_OPTIONS`, and any directive
//!   other than the four above: an unknown directive is a rejection, so this
//!   parser does not claim to read a file that uses them.
//! * How `nsswitch.conf` chooses between files and DNS.
//!
//! # The corpus
//!
//! Hand-written cases for every rule above; a deterministic pseudo-random fan
//! (systematic truncations, systematic single-byte flips of a valid file of each
//! format, then seeded random edits and random bytes) because a parser that
//! agrees on the tidy cases is exactly the parser that comes apart on the
//! others. The fan is an LCG, not a random source, so the corpus is the same
//! corpus on every run and a failure is reproducible from the report.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every reason either parser can print. The corpus must exercise all of them,
/// or it is not covering the contract it claims to.
const REASONS: [&str; 9] = [
    "hosts-bad-address",
    "hosts-no-name",
    "hosts-bad-name",
    "resolv-unknown-keyword",
    "resolv-missing-argument",
    "resolv-bad-address",
    "resolv-extra-argument",
    "resolv-bad-search",
    "resolv-bad-option",
];

/// Which of the two files a case is. The driver calls the matching entry point;
/// naming the format in the case is what keeps the corpus honest about which
/// rules it is exercising.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Hosts,
    Resolv,
}

struct Case {
    what: &'static str,
    kind: Kind,
    text: Vec<u8>,
    /// `None`: only the two implementations are compared for this case.
    /// `Some("ok")`: the line must start with `ok `. `Some(line)`: the line
    /// must equal it exactly (used for the hand-written cases, so the format
    /// and the rules are pinned absolutely and not only differentially).
    expect: Option<&'static str>,
}

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn tmp(name: &str) -> PathBuf {
    let mut p = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    p.push(name);
    p
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---- the corpus -------------------------------------------------------------

fn corpus() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    // The files a fan is built from: every well-formed one of each format, so a
    // truncation or a flip is a small edit of something that parsed.
    let mut valid_hosts: Vec<Vec<u8>> = Vec::new();
    let mut valid_resolv: Vec<Vec<u8>> = Vec::new();
    let accept = |cases: &mut Vec<Case>,
                  valid: &mut Vec<Vec<u8>>,
                  what: &'static str,
                  kind: Kind,
                  text: &str,
                  expect: Option<&'static str>| {
        let text = text.as_bytes().to_vec();
        valid.push(text.clone());
        cases.push(Case {
            what,
            kind,
            text,
            expect,
        });
    };
    let reject = |cases: &mut Vec<Case>,
                  what: &'static str,
                  kind: Kind,
                  text: &str,
                  expect: Option<&'static str>| {
        cases.push(Case {
            what,
            kind,
            text: text.as_bytes().to_vec(),
            expect,
        });
    };

    // ---- hosts: the accepting side ----------------------------------------

    accept(
        &mut cases,
        &mut valid_hosts,
        "loopback, an inline comment and a tab-separated IPv6 line",
        Kind::Hosts,
        "127.0.0.1 localhost localhost.localdomain # the loopback\n::1\tlocalhost ip6-localhost\n",
        Some("ok hosts e=127.0.0.1|localhost,localhost.localdomain e=::1|localhost,ip6-localhost"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "blank lines and a comment-only line are skipped",
        Kind::Hosts,
        "\n# a comment\n   \n\t\n10.0.0.1 a\n",
        Some("ok hosts e=10.0.0.1|a"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "doubled tabs and spaces between fields",
        Kind::Hosts,
        "10.0.0.2\t\ta.example   b.example  \t\n",
        Some("ok hosts e=10.0.0.2|a.example,b.example"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "names are lowercased",
        Kind::Hosts,
        "10.0.0.3 HOST.Example B\n",
        Some("ok hosts e=10.0.0.3|host.example,b"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "duplicate entries and two addresses for one name are kept in order",
        Kind::Hosts,
        "1.2.3.4 a\n1.2.3.4 a\n5.6.7.8 a\n",
        Some("ok hosts e=1.2.3.4|a e=1.2.3.4|a e=5.6.7.8|a"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "CRLF",
        Kind::Hosts,
        "127.0.0.1 a\r\n1.2.3.4 b\r\n",
        Some("ok hosts e=127.0.0.1|a e=1.2.3.4|b"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "an empty file",
        Kind::Hosts,
        "",
        Some("ok hosts"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "a file of nothing but blank and comment lines",
        Kind::Hosts,
        "# nothing\n\n",
        Some("ok hosts"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "a comment with no trailing newline",
        Kind::Hosts,
        "1.2.3.4 a # tail",
        Some("ok hosts e=1.2.3.4|a"),
    );

    accept(
        &mut cases,
        &mut valid_hosts,
        "IPv6 canonical forms",
        Kind::Hosts,
        concat!(
            "2001:0db8:0000:0000:0000:0000:0000:0001\ta\n",
            "::1 b\n",
            ":: c\n",
            "0:0:1:0:0:0:0:1 d\n",
            "2001:DB8::1 e\n",
            "fe80:0:0:0:0:0:0:1 f\n",
            "1:2:3:4:5:6:7:8 g\n",
            "1:0:1:1:1:1:1:1 h\n",
            "1:0:0:1:1:1:0:0 i\n",
        ),
        Some(
            "ok hosts \
             e=2001:db8::1|a \
             e=::1|b \
             e=::|c \
             e=0:0:1::1|d \
             e=2001:db8::1|e \
             e=fe80::1|f \
             e=1:2:3:4:5:6:7:8|g \
             e=1:0:1:1:1:1:1:1|h \
             e=1::1:1:1:0:0|i",
        ),
    );

    // ---- hosts: the rejecting side ----------------------------------------

    reject(
        &mut cases,
        "an octet over 255",
        Kind::Hosts,
        "300.1.1.1 x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "a leading zero on an octet",
        Kind::Hosts,
        "010.0.0.1 x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "three octets",
        Kind::Hosts,
        "1.2.3 x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "five octets",
        Kind::Hosts,
        "1.2.3.4.5 x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "a trailing byte after the quad",
        Kind::Hosts,
        "1.2.3.4x y\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "a zone suffix on IPv6",
        Kind::Hosts,
        "fe80::1%eth0 x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "an embedded IPv4 tail",
        Kind::Hosts,
        "::ffff:1.2.3.4 x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "two gaps",
        Kind::Hosts,
        "1::2::3 x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "five hex digits in a group",
        Kind::Hosts,
        "12345:: x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "a trailing single colon",
        Kind::Hosts,
        "1:2:3:4:5:6:7:8: x\n",
        Some("reject hosts-bad-address"),
    );
    reject(
        &mut cases,
        "a non-hex group",
        Kind::Hosts,
        "1:2:3:4:5:6:7:g x\n",
        Some("reject hosts-bad-address"),
    );

    reject(
        &mut cases,
        "an address with no name",
        Kind::Hosts,
        "1.2.3.4\n",
        Some("reject hosts-no-name"),
    );
    reject(
        &mut cases,
        "an address, a trailing space and a comment",
        Kind::Hosts,
        "1.2.3.4 # nobody\n",
        Some("reject hosts-no-name"),
    );
    reject(
        &mut cases,
        "a bare IPv6 address with no name",
        Kind::Hosts,
        "::\n",
        Some("reject hosts-no-name"),
    );

    reject(
        &mut cases,
        "a name that starts with a hyphen",
        Kind::Hosts,
        "1.2.3.4 -bad-\n",
        Some("reject hosts-bad-name"),
    );
    reject(
        &mut cases,
        "a name that ends with a hyphen",
        Kind::Hosts,
        "1.2.3.4 bad-\n",
        Some("reject hosts-bad-name"),
    );
    reject(
        &mut cases,
        "an underscore in a name",
        Kind::Hosts,
        "1.2.3.4 under_score\n",
        Some("reject hosts-bad-name"),
    );
    reject(
        &mut cases,
        "a trailing dot",
        Kind::Hosts,
        "1.2.3.4 foo.\n",
        Some("reject hosts-bad-name"),
    );
    reject(
        &mut cases,
        "an empty label",
        Kind::Hosts,
        "1.2.3.4 a..b\n",
        Some("reject hosts-bad-name"),
    );
    reject(
        &mut cases,
        "a bad name on a later line, after a good entry",
        Kind::Hosts,
        "1.2.3.4 good\n5.6.7.8 bad_name\n",
        Some("reject hosts-bad-name"),
    );
    reject(
        &mut cases,
        "a bad address on a later line, after a good entry",
        Kind::Hosts,
        "1.2.3.4 good\n5.6.7.8x better\n",
        Some("reject hosts-bad-address"),
    );
    {
        // A 64-byte label, one over the bound; the accepting side is the 63
        // below.
        let over = format!("1.2.3.4 {}\n", "a".repeat(64));
        cases.push(Case {
            what: "a label of 64 bytes",
            kind: Kind::Hosts,
            text: over.into_bytes(),
            expect: Some("reject hosts-bad-name"),
        });
        let at = format!("1.2.3.4 {}\n", "a".repeat(63));
        accept(
            &mut cases,
            &mut valid_hosts,
            "a label of 63 bytes is accepted",
            Kind::Hosts,
            &at,
            Some("ok"),
        );
    }

    // ---- resolv.conf: the accepting side ----------------------------------

    accept(
        &mut cases,
        &mut valid_resolv,
        "an IPv4 and an IPv6 nameserver, with a comment and a blank line",
        Kind::Resolv,
        "# a comment\nnameserver 1.1.1.1\n\nnameserver 2606:4700:4700::1111 # v6\n",
        Some(
            "ok ns=1.1.1.1,2606:4700:4700::1111 nsdrop=0 search=- searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "semantically empty",
        Kind::Resolv,
        "",
        Some(
            "ok ns=- nsdrop=0 search=- searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "a semicolon comment and a trailing semicolon comment",
        Kind::Resolv,
        "; comment\nnameserver 1.1.1.1 ; tail\n",
        Some(
            "ok ns=1.1.1.1 nsdrop=0 search=- searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "domain then search: the last one wins",
        Kind::Resolv,
        "domain example.com\nsearch a.example b.example\n",
        Some(
            "ok ns=- nsdrop=0 search=a.example,b.example searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "search then domain: the last one wins, and it is one element",
        Kind::Resolv,
        "search a.example b.example\ndomain example.com\n",
        Some(
            "ok ns=- nsdrop=0 search=example.com searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "search names are lowercased",
        Kind::Resolv,
        "search Example.COM\n",
        Some(
            "ok ns=- nsdrop=0 search=example.com searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "every known option, and two unknown ones are skipped and counted",
        Kind::Resolv,
        "options ndots:5 timeout:30 attempts:5 rotate edns0 no-tld-query\n",
        Some(
            "ok ns=- nsdrop=0 search=- searchdrop=0 \
             ndots=5 timeout=30 attempts=5 rotate=true unknown=2",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "the bounds of every option value",
        Kind::Resolv,
        "options ndots:0 timeout:1 attempts:1\noptions ndots:15 timeout:30 attempts:5\n",
        Some(
            "ok ns=- nsdrop=0 search=- searchdrop=0 \
             ndots=15 timeout=30 attempts=5 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "options accumulate across lines, and rotate stays set",
        Kind::Resolv,
        "options rotate\noptions ndots:3\noptions edns0 edns0\n",
        Some(
            "ok ns=- nsdrop=0 search=- searchdrop=0 \
             ndots=3 timeout=5 attempts=2 rotate=true unknown=2",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "an options line with nothing after it",
        Kind::Resolv,
        "options\n",
        Some(
            "ok ns=- nsdrop=0 search=- searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "five nameservers: three are kept, in file order, and two are dropped",
        Kind::Resolv,
        concat!(
            "nameserver 1.1.1.1\n",
            "nameserver 2.2.2.2\n",
            "nameserver 2606:4700:4700::1111\n",
            "nameserver 4.4.4.4\n",
            "nameserver 5.5.5.5\n",
        ),
        Some(
            "ok ns=1.1.1.1,2.2.2.2,2606:4700:4700::1111 nsdrop=2 search=- searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "eight search domains: six are kept and two are dropped",
        Kind::Resolv,
        "search a b c d e f g h\n",
        Some(
            "ok ns=- nsdrop=0 search=a,b,c,d,e,f searchdrop=2 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "a search list of six is exactly at the bound",
        Kind::Resolv,
        "search a b c d e f\n",
        Some(
            "ok ns=- nsdrop=0 search=a,b,c,d,e,f searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    accept(
        &mut cases,
        &mut valid_resolv,
        "CRLF",
        Kind::Resolv,
        "nameserver 1.1.1.1\r\nsearch a.example\r\n",
        Some(
            "ok ns=1.1.1.1 nsdrop=0 search=a.example searchdrop=0 \
             ndots=1 timeout=5 attempts=2 rotate=false unknown=0",
        ),
    );

    // ---- resolv.conf: the rejecting side ----------------------------------

    reject(
        &mut cases,
        "a misspelled directive",
        Kind::Resolv,
        "nameserverr 1.1.1.1\n",
        Some("reject resolv-unknown-keyword"),
    );
    reject(
        &mut cases,
        "a directive in uppercase",
        Kind::Resolv,
        "NAMESERVER 1.1.1.1\n",
        Some("reject resolv-unknown-keyword"),
    );
    reject(
        &mut cases,
        "a line that is not a directive at all",
        Kind::Resolv,
        "1.1.1.1\n",
        Some("reject resolv-unknown-keyword"),
    );
    reject(
        &mut cases,
        "a bad directive after a good one",
        Kind::Resolv,
        "nameserver 1.1.1.1\nsortlist 130.155.160.0/255.255.240.0\n",
        Some("reject resolv-unknown-keyword"),
    );

    reject(
        &mut cases,
        "nameserver with no address",
        Kind::Resolv,
        "nameserver\n",
        Some("reject resolv-missing-argument"),
    );
    reject(
        &mut cases,
        "domain with no argument",
        Kind::Resolv,
        "domain\n",
        Some("reject resolv-missing-argument"),
    );
    reject(
        &mut cases,
        "search with no argument",
        Kind::Resolv,
        "search\n",
        Some("reject resolv-missing-argument"),
    );

    reject(
        &mut cases,
        "a nameserver that is a hostname",
        Kind::Resolv,
        "nameserver dns.example.com\n",
        Some("reject resolv-bad-address"),
    );
    reject(
        &mut cases,
        "a nameserver with a bad octet",
        Kind::Resolv,
        "nameserver 300.1.1.1\n",
        Some("reject resolv-bad-address"),
    );
    reject(
        &mut cases,
        "a nameserver with an embedded IPv4 tail",
        Kind::Resolv,
        "nameserver ::ffff:1.1.1.1\n",
        Some("reject resolv-bad-address"),
    );

    reject(
        &mut cases,
        "nameserver with a second address",
        Kind::Resolv,
        "nameserver 1.1.1.1 2.2.2.2\n",
        Some("reject resolv-extra-argument"),
    );
    reject(
        &mut cases,
        "domain with two arguments",
        Kind::Resolv,
        "domain a.example b.example\n",
        Some("reject resolv-extra-argument"),
    );

    reject(
        &mut cases,
        "a lone dot as the domain",
        Kind::Resolv,
        "domain .\n",
        Some("reject resolv-bad-search"),
    );
    reject(
        &mut cases,
        "a search entry with an underscore",
        Kind::Resolv,
        "search a_b.example\n",
        Some("reject resolv-bad-search"),
    );
    reject(
        &mut cases,
        "the second search entry is bad",
        Kind::Resolv,
        "search good.example -bad.example\n",
        Some("reject resolv-bad-search"),
    );

    reject(
        &mut cases,
        "ndots over 15",
        Kind::Resolv,
        "options ndots:16\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "ndots with no value",
        Kind::Resolv,
        "options ndots:\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "ndots with a non-numeric value",
        Kind::Resolv,
        "options ndots:abc\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "ndots with a leading zero",
        Kind::Resolv,
        "options ndots:01\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "timeout 0",
        Kind::Resolv,
        "options timeout:0\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "timeout 31",
        Kind::Resolv,
        "options timeout:31\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "attempts 0",
        Kind::Resolv,
        "options attempts:0\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "attempts 6",
        Kind::Resolv,
        "options attempts:6\n",
        Some("reject resolv-bad-option"),
    );
    reject(
        &mut cases,
        "a bad option on a later options line",
        Kind::Resolv,
        "options rotate\noptions ndots:20\n",
        Some("reject resolv-bad-option"),
    );

    // ---- the deterministic fan --------------------------------------------
    //
    // An LCG, not a random source: the corpus has to be the same corpus on every
    // run and every machine, or a failure is not reproducible from the report.
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };

    // Systematic truncations: every prefix of the first few valid files of each
    // format. A truncated config file is what a half-written file or a short
    // read produces, and every cut point is a different read at the edge.
    for (kind, set) in [(Kind::Hosts, &valid_hosts), (Kind::Resolv, &valid_resolv)] {
        for start in 0..set.len().min(4) {
            let base = set[start].clone();
            for cut in 0..=base.len() {
                cases.push(Case {
                    what: "truncation",
                    kind,
                    text: base[..cut].to_vec(),
                    expect: None,
                });
            }
        }
    }

    // Systematic single-byte edits of one representative file of each format:
    // every position, flipped high bit and low bit. This is where a separator
    // becomes a token byte, a digit becomes a letter, and a comment introducer
    // stops being one.
    for (kind, base) in [(Kind::Hosts, valid_hosts[0].clone()), (Kind::Resolv, valid_resolv[0].clone())] {
        for i in 0..base.len() {
            for mask in [0x01u8, 0x80] {
                let mut m = base.clone();
                m[i] ^= mask;
                cases.push(Case {
                    what: "single-byte flip",
                    kind,
                    text: m,
                    expect: None,
                });
            }
        }
    }

    // Random truncations and multi-byte flips of the valid files.
    for _ in 0..1400 {
        let kind = if next() % 2 == 0 {
            Kind::Hosts
        } else {
            Kind::Resolv
        };
        let set = if kind == Kind::Hosts {
            &valid_hosts
        } else {
            &valid_resolv
        };
        // The empty file is a valid base for a truncation but not for an edit,
        // so a random edit picks a base that has at least one byte.
        let mut base = &set[next() % set.len()];
        while base.is_empty() {
            base = &set[next() % set.len()];
        }
        if next() % 2 == 0 {
            let cut = next() % (base.len() + 1);
            cases.push(Case {
                what: "random truncation",
                kind,
                text: base[..cut].to_vec(),
                expect: None,
            });
        } else {
            let mut m = base.clone();
            let edits = 1 + next() % 3;
            for _ in 0..edits {
                let at = next() % m.len();
                m[at] = next() as u8;
            }
            cases.push(Case {
                what: "random flip",
                kind,
                text: m,
                expect: None,
            });
        }
    }

    // Random bytes from scratch, which lands on the line and token rules more
    // often than anything else.
    for _ in 0..500 {
        let len = next() % 64;
        let text: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let kind = if next() % 2 == 0 {
            Kind::Hosts
        } else {
            Kind::Resolv
        };
        cases.push(Case {
            what: "random bytes",
            kind,
            text,
            expect: None,
        });
    }

    cases
}

// ---- the two drivers --------------------------------------------------------

fn c_driver(cases: &[Case]) -> String {
    let mut out = String::new();
    out.push_str("/* Generated by kelpc/tests/resolver_conf.rs: one line per case. */\n");
    out.push_str(&format!(
        "#include \"{}\"\n",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime/resolver_conf_ref.c")
            .display()
    ));
    out.push_str("#include <stdio.h>\n#include <stddef.h>\n\nint main(void) {\n");
    out.push_str("    char line[1 << 20];\n");
    for c in cases {
        let entry = match c.kind {
            Kind::Hosts => "kelp_hosts_parse",
            Kind::Resolv => "kelp_resolv_parse",
        };
        out.push_str("    {\n");
        if c.text.is_empty() {
            out.push_str("        static const unsigned char data[1] = { 0 };\n");
        } else {
            let mut bytes = String::new();
            for (i, b) in c.text.iter().enumerate() {
                if i % 16 == 0 {
                    bytes.push_str("\n            ");
                }
                bytes.push_str(&format!("0x{b:02x}, "));
            }
            out.push_str(&format!(
                "        static const unsigned char data[] = {{{bytes}\n        }};\n"
            ));
        }
        out.push_str(&format!(
            "        static const size_t data_len = {};\n",
            c.text.len()
        ));
        out.push_str(&format!(
            "        const char *err = {entry}(data, data_len, line, sizeof line);\n"
        ));
        out.push_str("        if (err != NULL) printf(\"reject %s\\n\", err);\n");
        out.push_str("        else printf(\"%s\\n\", line);\n");
        out.push_str("    }\n");
    }
    out.push_str("    return 0;\n}\n");
    out
}

/// The Kelp driver. A Kelp string literal is UTF-8 and cannot carry an arbitrary
/// byte, so each case passes its hex and the driver decodes it: the same bytes
/// reach both implementations.
fn kelp_driver(cases: &[Case]) -> String {
    let mut out = String::new();
    out.push_str("// Generated by kelpc/tests/resolver_conf.rs: one line per case.\n");
    out.push_str("impl \"resolver_conf\"\nimpl \"std/str\"\n\n");
    out.push_str("fun hex_val(c: u8).R: u8 {\n");
    out.push_str("    if c >= 48 && c <= 57 {\n        return c - 48\n    }\n    return c - 87\n}\n\n");
    out.push_str("fun hex_to_bytes(s: &str).R: Bytes {\n");
    out.push_str("    val n = str.len(s)\n    var out = Bytes.empty()\n    var i: usize = 0\n");
    out.push_str("    while i < n {\n");
    out.push_str("        val hi = hex_val(str.byte(s, i))\n");
    out.push_str("        val lo = hex_val(str.byte(s, i + 1))\n");
    out.push_str("        out.push(hi * 16 + lo)\n        i = i + 2\n    }\n    return out\n}\n\n");
    out.push_str("fun main() {\n");
    for c in cases {
        let call = match c.kind {
            Kind::Hosts => "line_hosts",
            Kind::Resolv => "line_resolv",
        };
        out.push_str(&format!(
            "    print(resolver_conf.{call}(hex_to_bytes(\"{}\")))\n",
            hex(&c.text)
        ));
    }
    out.push_str("}\n");
    out
}

fn build_c_driver(cases: &[Case]) -> PathBuf {
    let src = tmp("resolver_conf_c.c");
    let bin = tmp("resolver_conf_c");
    std::fs::write(&src, c_driver(cases)).expect("writing the C driver");
    let out = Command::new("cc")
        .args(["-w", "-o"])
        .arg(&bin)
        .arg(&src)
        .output()
        .expect("failed to invoke cc for the corpus driver");
    assert!(
        out.status.success(),
        "cc could not build the reference driver:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Builds the Kelp side with the C backend. The backend is pinned for the same
/// reason `dns_parse.rs` pins it: this test is about the parser, not about which
/// backend is under test.
///
/// The generated driver is written into a directory of its own with a copy of
/// `resolver_conf.kp`, because `impl "..."` resolves against the entry file's
/// directory and the module is deliberately not a std module.
fn build_kelp_driver(cases: &[Case]) -> PathBuf {
    let dir = tmp("resolver_conf_kelp");
    std::fs::create_dir_all(&dir).expect("creating the Kelp driver directory");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/resolver_conf.kp"),
        dir.join("resolver_conf.kp"),
    )
    .expect("copying resolver_conf.kp next to the driver");
    let src = dir.join("driver.kp");
    let bin = dir.join("resolver_conf_kelp");
    std::fs::write(&src, kelp_driver(cases)).expect("writing the Kelp driver");
    let out = Command::new(kelpc_bin())
        .args(["build", "--backend=c"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "kelpc could not build the Kelp driver:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

fn run_driver(bin: &Path, who: &str) -> Vec<String> {
    let out = Command::new(bin).output().expect("failed to run the driver");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "the {who} driver exited with {:?} after {} lines:\n{}",
        out.status.code(),
        stdout.lines().count(),
        String::from_utf8_lossy(&out.stderr)
    );
    stdout.lines().map(|l| l.to_string()).collect()
}

#[test]
fn the_kelp_resolver_config_parser_agrees_with_the_c_reference_on_the_whole_corpus() {
    let cases = corpus();
    assert!(
        cases.len() > 1000,
        "the corpus is too small to mean anything: {} cases",
        cases.len()
    );

    let oracle = run_driver(&build_c_driver(&cases), "C reference");
    let port = run_driver(&build_kelp_driver(&cases), "Kelp");

    assert_eq!(
        oracle.len(),
        cases.len(),
        "the C driver printed {} lines for {} cases",
        oracle.len(),
        cases.len()
    );
    assert_eq!(
        port.len(),
        cases.len(),
        "the Kelp driver printed {} lines for {} cases",
        port.len(),
        cases.len()
    );

    // The hand-written cases are pinned absolutely, so a shared belief that
    // happens to be wrong in both implementations is caught here and not only
    // by agreement between them.
    for (i, case) in cases.iter().enumerate() {
        let Some(want) = case.expect else { continue };
        if want == "ok" {
            assert!(
                oracle[i].starts_with("ok "),
                "case {i} ({}): the reference says {:?}, expected an accepted line",
                case.what,
                oracle[i]
            );
        } else {
            assert_eq!(
                oracle[i], want,
                "case {i} ({}): the reference disagrees with the pinned expectation",
                case.what
            );
        }
    }

    // Both verdicts, and every rejection class. A corpus that only accepted, or
    // only rejected, would agree while proving nothing; and a class that stopped
    // being exercised would quietly fall out of the contract. The split is
    // checked per format as well, because a fan that stopped deciding one way
    // for `/etc/resolv.conf` and the other way for `/etc/hosts` would still pass
    // the combined counts.
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut seen: Vec<&str> = Vec::new();
    let mut hosts_split = (0usize, 0usize);
    let mut resolv_split = (0usize, 0usize);
    for (case, line) in cases.iter().zip(&oracle) {
        let split = match case.kind {
            Kind::Hosts => &mut hosts_split,
            Kind::Resolv => &mut resolv_split,
        };
        if line.starts_with("ok") {
            accepted += 1;
            split.0 += 1;
        } else if let Some(reason) = line.strip_prefix("reject ") {
            rejected += 1;
            split.1 += 1;
            if !seen.contains(&reason) {
                seen.push(reason);
            }
        } else {
            panic!("the reference printed a line that is neither verdict: {line:?}");
        }
    }
    assert!(
        accepted > 100 && rejected > 100,
        "{accepted} accepted / {rejected} rejected: the corpus has to decide both ways"
    );
    assert!(
        hosts_split.0 > 0 && hosts_split.1 > 0,
        "the hosts half of the corpus decided only one way: {} accepted, {} rejected",
        hosts_split.0,
        hosts_split.1
    );
    assert!(
        resolv_split.0 > 0 && resolv_split.1 > 0,
        "the resolv.conf half of the corpus decided only one way: {} accepted, {} rejected",
        resolv_split.0,
        resolv_split.1
    );
    let mut missing: Vec<&str> = REASONS
        .iter()
        .copied()
        .filter(|r| !seen.contains(r))
        .collect();
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "the corpus does not exercise these rejection classes: {missing:?}"
    );

    eprintln!(
        "resolver_conf: {} cases, {accepted} accepted, {rejected} rejected, {} reasons exercised \
         (hosts {} ok / {} reject, resolv {} ok / {} reject)",
        cases.len(),
        seen.len(),
        hosts_split.0,
        hosts_split.1,
        resolv_split.0,
        resolv_split.1
    );

    for (i, (case, (want, got))) in cases.iter().zip(oracle.iter().zip(&port)).enumerate() {
        assert_eq!(
            want, got,
            "case {i} ({}) disagrees:\n  C    {want}\n  Kelp {got}",
            case.what
        );
    }
}

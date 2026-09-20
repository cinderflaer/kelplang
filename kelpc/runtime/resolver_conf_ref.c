// ---- resolver configuration parsing: the C reference (MK7 §7 item 6, §8) ----
//
// This is the C oracle for `kelpc/runtime/resolver_conf.kp`, the Kelp port of
// the configuration half of the stub resolver: `/etc/hosts` and
// `/etc/resolv.conf`. Nothing includes it but `kelpc/tests/resolver_conf.rs`,
// which runs one corpus through both and compares every rendered line.
//
// §8's contract is "Kelp first, the C kept as the reference, prove it with a
// corpus", and §7 names the resolver as the largest single non-codegen item and
// the one most likely to be deferred. Its transport half needs a socket; its
// configuration half does not, and a config file an attacker can write is a
// config file that must not be able to walk a parser off its buffer. Every read
// here is bounds-checked, an address and a hostname are parsed by the strict
// rules the Kelp module documents, and a line that is not one of the four
// `resolv.conf` directives rejects the file rather than being skipped.
//
// Two entry points, one per format, each returning NULL and writing the
// canonical one-line rendering into `out`, or returning the wire name of the
// reason the file was refused. The driver prints `reject <reason>` in that
// case, so a partial rendering is never observed. The renderings are:
//
//   ok hosts[ e=<addr>|<name>,<name> ...]
//   ok ns=<list> nsdrop=<n> search=<list> searchdrop=<n> ndots=<n> timeout=<n>
//      attempts=<n> rotate=<true|false> unknown=<n>
//
// `<list>` is `,`-joined, or `-` when empty. The hosts line streams its entries
// as they are parsed, so its entry count is the number of `e=` groups; the
// resolv.conf line needs the whole file first, and its storage is fixed because
// every value it holds is bounded by a rule (`MAXNS` 3 nameservers, `MAXDNSRCH`
// 6 search domains, a 255-byte hostname, a 39-byte IPv6 address). The caller's
// `out` buffer bounds the rest: a rendering longer than it is truncated rather
// than returned, which is the one case the two implementations could disagree
// on, and why the corpus driver's buffer is a megabyte for cases whose largest
// rendering is a few hundred bytes.
#include <stdbool.h>
#include <stddef.h>

// glibc's limits, and the defaults `res_init` starts from.
#define MAXNS 3
#define MAXDNSRCH 6

// An append cursor over the caller's line buffer.
typedef struct {
    char *buf;
    size_t cap;
    size_t n;
} out_t;

static void put_c(out_t *o, char c) {
    if (o->n + 1 < o->cap) {
        o->buf[o->n++] = c;
        o->buf[o->n] = 0;
    }
}

static void put_s(out_t *o, const char *s) {
    while (*s != 0) {
        put_c(o, *s++);
    }
}

static void put_u64(out_t *o, unsigned long long v) {
    char tmp[24];
    int i = 0;
    if (v == 0) {
        put_c(o, '0');
        return;
    }
    while (v != 0) {
        tmp[i++] = (char)('0' + (v % 10));
        v /= 10;
    }
    while (i > 0) {
        put_c(o, tmp[--i]);
    }
}

static char hex_char(unsigned n) {
    return (char)(n < 10 ? ('0' + n) : ('a' + (n - 10)));
}

// ---- byte predicates --------------------------------------------------------

// Space or tab, the two separators both formats use.
static bool is_blank(unsigned b) {
    return b == 32 || b == 9;
}

// `0`..`9`.
static bool is_digit(unsigned b) {
    return b >= 48 && b <= 57;
}

// `0`..`9`, `a`..`z`, `A`..`Z`.
static bool is_alnum(unsigned b) {
    return is_digit(b) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122);
}

// `0`..`9`, `a`..`f`, `A`..`F`.
static bool is_hex(unsigned b) {
    return is_digit(b) || (b >= 97 && b <= 102) || (b >= 65 && b <= 70);
}

// A hex digit's value. The caller has checked `is_hex`.
static unsigned hex_val(unsigned b) {
    if (b >= 48 && b <= 57) {
        return b - 48;
    }
    if (b >= 97 && b <= 102) {
        return b - 87;
    }
    return b - 55;
}

// ASCII lowercase, the only case folding both formats need.
static unsigned lower(unsigned b) {
    if (b >= 65 && b <= 90) {
        return b + 32;
    }
    return b;
}

// ---- lines and tokens -------------------------------------------------------

typedef struct {
    size_t start;
    size_t len;
} span_t;

typedef struct {
    bool found;
    span_t span;
    size_t at;
} scan_t;

typedef struct {
    bool ok;
    unsigned value;
} number_t;

// The end of the logical line that starts at `start`, whose newline (if any) is
// at `nl`: the first comment introducer or `nl`, whichever comes first, with a
// trailing carriage return dropped so a CRLF file parses exactly like an LF one.
// `semi` admits `;` as a comment introducer, which `/etc/resolv.conf` does and
// `/etc/hosts` does not.
static size_t line_end(const unsigned char *d, size_t start, size_t nl, bool semi) {
    size_t e = nl;
    size_t i = start;
    bool done = false;
    while (!done && i < e) {
        unsigned b = d[i];
        if (b == 35 || (semi && b == 59)) {
            e = i;
            done = true;
        } else {
            i++;
        }
    }
    if (e > start && d[e - 1] == 13) {
        e--;
    }
    return e;
}

// The next blank-separated token at or after `at`, inside `[at, end)`. An empty
// token cannot be produced: runs of blanks are skipped, and a token ends at the
// next blank or at `end`.
static scan_t scan(const unsigned char *d, size_t at, size_t end) {
    scan_t s;
    size_t i = at;
    while (i < end && is_blank(d[i])) {
        i++;
    }
    if (i >= end) {
        s.found = false;
        s.span.start = end;
        s.span.len = 0;
        s.at = i;
        return s;
    }
    size_t begin = i;
    while (i < end && !is_blank(d[i])) {
        i++;
    }
    s.found = true;
    s.span.start = begin;
    s.span.len = i - begin;
    s.at = i;
    return s;
}

// Whether the token is exactly `word`. Keywords are matched case-sensitively,
// which is what glibc's `strncmp` does.
static bool token_is(const unsigned char *d, size_t start, size_t len, const char *word) {
    size_t i = 0;
    while (word[i] != 0) {
        if (i >= len || d[start + i] != (unsigned char)word[i]) {
            return false;
        }
        i++;
    }
    return i == len;
}

// Whether the token begins with `word`. The token may be exactly `word`: the
// caller decides whether a missing remainder is a value error or not.
static bool token_has_prefix(const unsigned char *d, size_t start, size_t len, const char *word) {
    size_t i = 0;
    while (word[i] != 0) {
        if (i >= len || d[start + i] != (unsigned char)word[i]) {
            return false;
        }
        i++;
    }
    return true;
}

// The decimal number in the token after `prefix` bytes of it: 1 to 3 digits, no
// leading zero on a multi-digit value and no sign, so a value a config writer
// and a reader could disagree about (`010`) is a rejection rather than a guess.
static number_t token_number(const unsigned char *d, size_t start, size_t len, size_t prefix) {
    number_t n;
    n.ok = false;
    n.value = 0;
    if (len <= prefix) {
        return n;
    }
    size_t count = len - prefix;
    if (count > 3) {
        return n;
    }
    if (count > 1 && d[start + prefix] == 48) {
        return n;
    }
    unsigned v = 0;
    for (size_t i = 0; i < count; i++) {
        unsigned b = d[start + prefix + i];
        if (!is_digit(b)) {
            return n;
        }
        v = v * 10 + (b - 48);
    }
    n.ok = true;
    n.value = v;
    return n;
}

// ---- addresses --------------------------------------------------------------
//
// The canonical spelling is written into `o`, which may hold a partial rendering
// when the answer is false: a refused case prints `reject <reason>` and never
// the rendered line, so a partial rendering is never observed.
//
// The rules are the strict ones the Kelp module documents: a dotted quad of four
// 1-to-3-digit octets each at most 255 with no leading zero, or eight IPv6
// groups with `::` at most once and no embedded IPv4 tail. `inet_aton` reads
// `010.0.0.1` as octal and `1.2.3` as a class-A form; a resolver that trusted
// that spelling would let a config file mean something its writer did not.

static bool has_colon(const unsigned char *d, size_t start, size_t len) {
    for (size_t i = start; i < start + len; i++) {
        if (d[i] == 58) {
            return true;
        }
    }
    return false;
}

// One IPv6 group, lowercase, without leading zeros.
static void push_hex_group(out_t *o, unsigned v) {
    bool started = false;
    for (size_t i = 0; i < 4; i++) {
        unsigned shift = (unsigned)((3 - i) * 4);
        unsigned nib = (v >> shift) & 15;
        if (nib != 0) {
            started = true;
        }
        if (started) {
            put_c(o, hex_char(nib));
        }
    }
    if (!started) {
        put_c(o, '0');
    }
}

// A dotted quad. `parts != 4` covers a short form (`1.2.3`) and an over-long one
// (`1.2.3.4.5`), and `i != end` covers a trailing byte that is neither digit nor
// dot.
static bool write_ipv4(const unsigned char *d, size_t start, size_t len, out_t *o) {
    size_t end = start + len;
    size_t i = start;
    unsigned parts = 0;
    bool ok = true;
    while (ok && parts < 4) {
        if (parts > 0) {
            if (i >= end || d[i] != 46) {
                ok = false;
            } else {
                i++;
            }
        }
        if (ok) {
            size_t s = i;
            while (i < end && is_digit(d[i])) {
                i++;
            }
            size_t count = i - s;
            if (count == 0 || count > 3) {
                ok = false;
            } else if (count > 1 && d[s] == 48) {
                ok = false;
            } else {
                unsigned v = 0;
                for (size_t k = s; k < i; k++) {
                    v = v * 10 + (d[k] - 48);
                }
                if (v > 255) {
                    ok = false;
                } else {
                    if (parts > 0) {
                        put_c(o, '.');
                    }
                    put_u64(o, v);
                    parts++;
                }
            }
        }
    }
    return ok && parts == 4 && i == end;
}

// An IPv6 address. One group is 1 to 4 hex digits, `::` appears at most once, a
// single `:` must have a group on both sides, and after the gap is expanded
// there must be exactly eight groups. A zone suffix (`%eth0`) and an embedded
// IPv4 tail (`::ffff:1.2.3.4`) are refused.
static bool write_ipv6(const unsigned char *d, size_t start, size_t len, out_t *o) {
    size_t end = start + len;
    unsigned groups[8];
    unsigned count = 0;
    size_t gap_at = 0;
    bool seen_gap = false;
    size_t i = start;
    // `state` is 0 when a field is required here (the start of the address), 1
    // when a separator is required (a field was just read) and 2 when a field is
    // optional (a `::` was just read). A single `:` is only a separator between
    // two fields, so a run of one colon at either end is what state 0 at the end
    // of the walk refuses.
    unsigned state = 0;
    bool ok = true;
    while (ok && i < end) {
        if (state == 1) {
            if (d[i] != 58) {
                ok = false;
            } else {
                i++;
                if (i < end && d[i] == 58 && !seen_gap) {
                    seen_gap = true;
                    gap_at = count;
                    i++;
                    state = 2;
                } else {
                    state = 0;
                }
            }
        } else {
            unsigned v = 0;
            size_t digits = 0;
            while (i < end && digits < 4 && is_hex(d[i])) {
                v = v * 16 + hex_val(d[i]);
                i++;
                digits++;
            }
            if (digits > 0) {
                if (count >= 8) {
                    ok = false;
                } else {
                    groups[count++] = v;
                    state = 1;
                }
            } else if (state == 0 && i + 1 < end && d[i] == 58 && d[i + 1] == 58 && !seen_gap) {
                seen_gap = true;
                gap_at = count;
                i += 2;
                state = 2;
            } else {
                ok = false;
            }
        }
    }
    if (state == 0) {
        ok = false;
    }
    size_t gap_len = 0;
    if (ok) {
        if (seen_gap) {
            // `::` stands for at least one group; eight or more is a refusal,
            // not an empty gap.
            if (count >= 8) {
                ok = false;
            } else {
                gap_len = 8 - count;
            }
        } else if (count != 8) {
            ok = false;
        }
    }
    if (!ok) {
        return false;
    }
    unsigned full[8];
    for (size_t j = 0; j < 8; j++) {
        if (j < gap_at) {
            full[j] = groups[j];
        } else if (j < gap_at + gap_len) {
            full[j] = 0;
        } else {
            full[j] = groups[j - gap_len];
        }
    }
    // RFC 5952's compressible run: the longest run of two or more zero groups,
    // and the leftmost one when two runs are the same length.
    size_t best_at = 8;
    size_t best_len = 0;
    size_t run_at = 0;
    size_t run_len = 0;
    for (size_t k = 0; k < 8; k++) {
        if (full[k] == 0) {
            if (run_len == 0) {
                run_at = k;
            }
            run_len++;
        } else {
            if (run_len > best_len) {
                best_len = run_len;
                best_at = run_at;
            }
            run_len = 0;
        }
    }
    if (run_len > best_len) {
        best_len = run_len;
        best_at = run_at;
    }
    // `sep` is whether a `:` is owed before whatever is written next: the first
    // `:` of a `::` separates the same way a group separator does, so the group
    // after a gap needs none.
    bool sep = false;
    for (size_t m = 0; m < 8;) {
        if (m == best_at && best_len >= 2) {
            put_c(o, ':');
            put_c(o, ':');
            sep = false;
            m = best_at + best_len;
        } else {
            if (sep) {
                put_c(o, ':');
            }
            push_hex_group(o, full[m]);
            sep = true;
            m++;
        }
    }
    return true;
}

// Whether the token holds an address, and its canonical spelling if it does.
static bool write_address(const unsigned char *d, size_t start, size_t len, out_t *o) {
    if (len == 0) {
        return false;
    }
    if (has_colon(d, start, len)) {
        return write_ipv6(d, start, len, o);
    }
    return write_ipv4(d, start, len, o);
}

// ---- hostnames --------------------------------------------------------------
//
// One name rule for both files. RFC 1123's preferred syntax, and no more: 1 to
// 63 bytes per label of ASCII letters, digits and hyphen, a hyphen not first or
// last in a label, at least one label, at most 255 bytes in all, and no trailing
// dot. An underscore, a leading or trailing hyphen, a dot at either end (`foo.`)
// and every non-ASCII byte are refusals, because a name that is not one thing
// means a lookup that has more than one answer. A trailing dot is DNS's spelling
// for "absolute"; v1 does not model a search suffix yet, so the two spellings
// are not merged here.
//
// The lowercased name is written into `o`, which may hold a partial rendering
// when the answer is false (see `write_address`).
static bool write_hostname(const unsigned char *d, size_t start, size_t len, out_t *o) {
    if (len == 0 || len > 255) {
        return false;
    }
    size_t end = start + len;
    size_t i = start;
    size_t label = 0;
    bool ok = true;
    while (ok && i < end) {
        unsigned b = d[i];
        if (b == 46) {
            if (label == 0 || d[i - 1] == 45) {
                ok = false;
            } else {
                put_c(o, '.');
                label = 0;
            }
        } else if (is_alnum(b) || (b == 45 && label > 0)) {
            put_c(o, (char)lower(b));
            label++;
            if (label > 63) {
                ok = false;
            }
        } else {
            ok = false;
        }
        i++;
    }
    // A hyphen at either end of the whole name, and a label that ended with one
    // before a dot, are refusals as much as a hyphen that starts a label is.
    return ok && label != 0 && d[end - 1] != 45;
}

// ---- /etc/hosts -------------------------------------------------------------

// Every line of the file, refusing the first line that is not one. A blank line
// and a comment-only line are not lines in this sense: they are skipped.
//
// A bad line rejects the *file* rather than being skipped: a hosts file with a
// mangled line is a hosts file whose author does not know what it says, and a
// resolver that quietly ignores it answers a query with the wrong address. The
// reason names the class of the first bad line, not which line it was.
//
// The entries stream into `o` as they are parsed, which is why the line carries
// no entry count: the number of `e=` groups is the count, and a file of any size
// renders without buffering it.
const char *kelp_hosts_parse(const unsigned char *d, size_t len, char *outbuf, size_t cap) {
    out_t o = { outbuf, cap, 0 };
    outbuf[0] = 0;
    put_s(&o, "ok hosts");
    size_t pos = 0;
    for (;;) {
        size_t nl = pos;
        bool found = false;
        while (!found && nl < len) {
            if (d[nl] == 10) {
                found = true;
            } else {
                nl++;
            }
        }
        size_t end = line_end(d, pos, nl, false);
        scan_t t0 = scan(d, pos, end);
        if (t0.found) {
            char addr[48];
            out_t sub = { addr, sizeof addr, 0 };
            addr[0] = 0;
            if (!write_address(d, t0.span.start, t0.span.len, &sub)) {
                return "hosts-bad-address";
            }
            scan_t t = scan(d, t0.at, end);
            if (!t.found) {
                return "hosts-no-name";
            }
            // The address is rendered into a buffer of its own first, because
            // the entry marker has to precede it on the line and the address
            // must be validated before the marker is written.
            put_s(&o, " e=");
            put_s(&o, addr);
            put_c(&o, '|');
            while (t.found) {
                if (!write_hostname(d, t.span.start, t.span.len, &o)) {
                    return "hosts-bad-name";
                }
                scan_t next = scan(d, t.at, end);
                if (next.found) {
                    put_c(&o, ',');
                }
                t = next;
            }
        }
        if (!found) {
            break;
        }
        pos = nl + 1;
    }
    return NULL;
}

// ---- /etc/resolv.conf -------------------------------------------------------

// Copy `n` bytes and terminate. The destination is one of the bounded stores
// below, and `n` is inside its capacity because the value was parsed by a rule
// that bounds it.
static void copy_text(char *dst, const char *src, size_t n) {
    for (size_t i = 0; i < n; i++) {
        dst[i] = src[i];
    }
    dst[n] = 0;
}

// Which known option the token is: 1 `rotate`, 2 `ndots:`, 3 `timeout:`,
// 4 `attempts:`, 0 anything else. A known option whose value is malformed is a
// rejection at the call site; an unknown one is skipped there.
static unsigned write_option(const unsigned char *d, size_t start, size_t len) {
    if (token_is(d, start, len, "rotate")) {
        return 1;
    }
    if (token_has_prefix(d, start, len, "ndots:")) {
        return 2;
    }
    if (token_has_prefix(d, start, len, "timeout:")) {
        return 3;
    }
    if (token_has_prefix(d, start, len, "attempts:")) {
        return 4;
    }
    return 0;
}

// `nameserver`, `domain`, `search` and `options`, in the order glibc's
// `res_init` reads them. `domain` and `search` are the same field: the last
// instance of either wins, whether the list has one element or six, which is
// what resolv.conf(5) says and what glibc does.
//
// A line that is not one of the four keywords rejects the file. glibc ignores an
// unknown directive; a resolver that skips a directive it does not understand
// is a resolver whose behaviour nobody can predict from the file, so this parser
// refuses instead.
const char *kelp_resolv_parse(const unsigned char *d, size_t len, char *outbuf, size_t cap) {
    // A hostname is at most 255 bytes and an IPv6 address at most 39, so both
    // stores are sized to hold the longest a rule allows, and no file can make
    // the parser allocate.
    char ns_text[MAXNS][48];
    size_t ns_count = 0;
    size_t ns_dropped = 0;
    char search_text[MAXDNSRCH][256];
    size_t search_count = 0;
    size_t search_dropped = 0;
    unsigned ndots = 1;
    unsigned timeout = 5;
    unsigned attempts = 2;
    bool rotate = false;
    size_t unknown = 0;

    size_t pos = 0;
    for (;;) {
        size_t nl = pos;
        bool found = false;
        while (!found && nl < len) {
            if (d[nl] == 10) {
                found = true;
            } else {
                nl++;
            }
        }
        size_t end = line_end(d, pos, nl, true);
        scan_t t0 = scan(d, pos, end);
        if (t0.found) {
            size_t kw = t0.span.start;
            size_t klen = t0.span.len;
            if (token_is(d, kw, klen, "nameserver")) {
                scan_t t = scan(d, t0.at, end);
                if (!t.found) {
                    return "resolv-missing-argument";
                }
                scan_t more = scan(d, t.at, end);
                if (more.found) {
                    return "resolv-extra-argument";
                }
                char addr[48];
                out_t sub = { addr, sizeof addr, 0 };
                addr[0] = 0;
                if (!write_address(d, t.span.start, t.span.len, &sub)) {
                    return "resolv-bad-address";
                }
                if (ns_count < MAXNS) {
                    copy_text(ns_text[ns_count], addr, sub.n);
                    ns_count++;
                } else {
                    ns_dropped++;
                }
            } else if (token_is(d, kw, klen, "domain")) {
                scan_t t = scan(d, t0.at, end);
                if (!t.found) {
                    return "resolv-missing-argument";
                }
                scan_t more = scan(d, t.at, end);
                if (more.found) {
                    return "resolv-extra-argument";
                }
                out_t sub = { search_text[0], sizeof search_text[0], 0 };
                search_text[0][0] = 0;
                if (!write_hostname(d, t.span.start, t.span.len, &sub)) {
                    return "resolv-bad-search";
                }
                search_count = 1;
                search_dropped = 0;
            } else if (token_is(d, kw, klen, "search")) {
                scan_t rest = scan(d, t0.at, end);
                if (!rest.found) {
                    return "resolv-missing-argument";
                }
                search_count = 0;
                search_dropped = 0;
                while (rest.found) {
                    char name[256];
                    out_t sub = { name, sizeof name, 0 };
                    name[0] = 0;
                    if (!write_hostname(d, rest.span.start, rest.span.len, &sub)) {
                        return "resolv-bad-search";
                    }
                    if (search_count < MAXDNSRCH) {
                        copy_text(search_text[search_count], name, sub.n);
                        search_count++;
                    } else {
                        search_dropped++;
                    }
                    rest = scan(d, rest.at, end);
                }
            } else if (token_is(d, kw, klen, "options")) {
                scan_t rest = scan(d, t0.at, end);
                while (rest.found) {
                    unsigned what = write_option(d, rest.span.start, rest.span.len);
                    if (what == 1) {
                        rotate = true;
                    } else if (what == 2) {
                        number_t num = token_number(d, rest.span.start, rest.span.len, 6);
                        if (!num.ok || num.value > 15) {
                            return "resolv-bad-option";
                        }
                        ndots = num.value;
                    } else if (what == 3) {
                        number_t num = token_number(d, rest.span.start, rest.span.len, 8);
                        if (!num.ok || num.value < 1 || num.value > 30) {
                            return "resolv-bad-option";
                        }
                        timeout = num.value;
                    } else if (what == 4) {
                        number_t num = token_number(d, rest.span.start, rest.span.len, 9);
                        if (!num.ok || num.value < 1 || num.value > 5) {
                            return "resolv-bad-option";
                        }
                        attempts = num.value;
                    } else {
                        // An option glibc does not know: skipped, and counted.
                        // The count is all that is kept, so no field of the
                        // parsed result can be made unbounded by the file.
                        unknown++;
                    }
                    rest = scan(d, rest.at, end);
                }
            } else {
                return "resolv-unknown-keyword";
            }
        }
        if (!found) {
            break;
        }
        pos = nl + 1;
    }

    out_t o = { outbuf, cap, 0 };
    outbuf[0] = 0;
    put_s(&o, "ok ns=");
    if (ns_count == 0) {
        put_c(&o, '-');
    }
    for (size_t i = 0; i < ns_count; i++) {
        if (i > 0) {
            put_c(&o, ',');
        }
        put_s(&o, ns_text[i]);
    }
    put_s(&o, " nsdrop=");
    put_u64(&o, ns_dropped);
    put_s(&o, " search=");
    if (search_count == 0) {
        put_c(&o, '-');
    }
    for (size_t i = 0; i < search_count; i++) {
        if (i > 0) {
            put_c(&o, ',');
        }
        put_s(&o, search_text[i]);
    }
    put_s(&o, " searchdrop=");
    put_u64(&o, search_dropped);
    put_s(&o, " ndots=");
    put_u64(&o, ndots);
    put_s(&o, " timeout=");
    put_u64(&o, timeout);
    put_s(&o, " attempts=");
    put_u64(&o, attempts);
    put_s(&o, " rotate=");
    put_s(&o, rotate ? "true" : "false");
    put_s(&o, " unknown=");
    put_u64(&o, unknown);
    return NULL;
}

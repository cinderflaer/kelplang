// ---- DNS message parsing: the C reference (MK7 §7 item 6, §8) ----------------
//
// This is the C oracle for `kelpc/runtime/dns_parse.kp`, the Kelp port of the
// message-parsing half of the stub resolver. Nothing includes it but
// `kelpc/tests/dns_parse.rs`, which runs one corpus through both and compares
// every rendered line.
//
// §8's contract is "Kelp first, the C kept as the reference, prove it with a
// corpus", and §7 names the resolver as the item most likely to be deferred and
// the hardest to get right. The failure that costs the most is not a wrong
// answer on a tidy reply; it is a malformed one that walks a compression
// pointer off the end of the buffer, or loops on it. So every read here is
// bounds-checked, a pointer must point strictly backwards, and a count the
// message cannot hold is refused before an entry is read.
//
// `kelp_dns_parse` returns NULL and writes the canonical one-line rendering
// into `out`, or returns the wire name of the reason the message was refused.
// The driver prints `reject <reason>` in that case, so a partial rendering is
// never observed. The rendering is the contract the corpus compares:
//
//   ok id=<dec> flags=<dec> qd=<dec> an=<dec> ns=<dec> ar=<dec> q=<...> rr=<...>
//
// with `q` = `<name>:<qtype>:<qclass>` and
// `rr` = `<sec>:<name>:<rtype>:<rclass>:<ttl>:<rdata>`. Names are decompressed
// and rendered as their labels' bytes in lowercase hex joined by `.` (root is
// `-`), so no control byte in a name can break the one-line-per-case shape.
// `rdata` is a dotted quad for A, eight 4-hex groups joined by `:` for AAAA,
// and `-` for every other type, which is skipped by RDLENGTH.
#include <stdbool.h>
#include <stddef.h>

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

static char hex_digit(unsigned n) {
    return (char)(n < 10 ? ('0' + n) : ('a' + (n - 10)));
}

// The name at `start`, decompressed and appended to `o`, with `*next` set to
// the offset just past it in its own section. Returns NULL, or the reason.
//
// A pointer is two octets with the top two bits set and must point strictly
// backwards (a self or forward pointer is refused before it is followed, which
// makes a loop impossible rather than merely unlikely). A pointer target past
// the end is refused; a label length octet with either reserved top bit set
// (`64..191`) is refused; and a name whose decompressed length would exceed 255
// octets, terminator included, is refused.
static const char *read_name(
    const unsigned char *d, size_t len, size_t start, out_t *o, size_t *next) {
    size_t pos = start;
    size_t total = 0;
    size_t hops = 0;
    bool jumped = false;
    bool wrote_label = false;
    *next = start;
    for (;;) {
        if (pos >= len) {
            return "truncated";
        }
        unsigned b = d[pos];
        if ((b & 0xC0) == 0xC0) {
            if (pos + 1 >= len) {
                return "truncated";
            }
            size_t target = (size_t)(b & 0x3F) * 256 + d[pos + 1];
            if (target >= len) {
                return "pointer-oob";
            }
            if (target >= pos) {
                return "pointer-backward";
            }
            if (!jumped) {
                *next = pos + 2;
                jumped = true;
            }
            // A strictly-backward chain cannot cycle, so this bound is never
            // reached; it is the guard that keeps that claim true if the rule
            // above is ever loosened.
            if (++hops > 128) {
                return "pointer-backward";
            }
            pos = target;
        } else if ((b & 0xC0) != 0) {
            return "bad-label";
        } else if (b == 0) {
            if (!jumped) {
                *next = pos + 1;
            }
            break;
        } else {
            size_t l = b;
            if (pos + 1 + l > len) {
                return "label-oob";
            }
            total += l + 1;
            if (total + 1 > 255) {
                return "name-too-long";
            }
            if (wrote_label) {
                put_c(o, '.');
            }
            wrote_label = true;
            for (size_t k = 0; k < l; k++) {
                unsigned octet = d[pos + 1 + k];
                put_c(o, hex_digit(octet >> 4));
                put_c(o, hex_digit(octet & 15));
            }
            pos += 1 + l;
        }
    }
    if (!wrote_label) {
        put_c(o, '-');
    }
    return NULL;
}

// The address bytes at `off`, appended: a dotted quad for A, eight 4-hex groups
// for AAAA. The caller has already checked `rdlen` against the type.
static void put_address(out_t *o, const unsigned char *d, size_t off, unsigned type) {
    if (type == 1) {
        for (size_t k = 0; k < 4; k++) {
            if (k > 0) {
                put_c(o, '.');
            }
            put_u64(o, d[off + k]);
        }
        return;
    }
    for (size_t g = 0; g < 8; g++) {
        if (g > 0) {
            put_c(o, ':');
        }
        for (size_t h = 0; h < 2; h++) {
            unsigned octet = d[off + g * 2 + h];
            put_c(o, hex_digit(octet >> 4));
            put_c(o, hex_digit(octet & 15));
        }
    }
}

static const char *section_name(unsigned section) {
    return section == 0 ? "an" : (section == 1 ? "ns" : "ar");
}

// Parses `d[0..len)` into the canonical line in `outbuf`, or returns the reason.
const char *kelp_dns_parse(const unsigned char *d, size_t len, char *outbuf, size_t cap) {
    out_t o = { outbuf, cap, 0 };
    outbuf[0] = 0;
    if (len < 12) {
        return "short-header";
    }
    unsigned id = (unsigned)((d[0] << 8) | d[1]);
    unsigned flags = (unsigned)((d[2] << 8) | d[3]);
    unsigned qd = (unsigned)((d[4] << 8) | d[5]);
    unsigned an = (unsigned)((d[6] << 8) | d[7]);
    unsigned ns = (unsigned)((d[8] << 8) | d[9]);
    unsigned ar = (unsigned)((d[10] << 8) | d[11]);
    // A count the message cannot hold is refused before a single entry is read.
    // A question is at least 5 bytes (a root name, type, class) and a record at
    // least 11 (root name, type, class, TTL, RDLENGTH).
    if ((size_t)12 + (size_t)qd * 5 + ((size_t)an + ns + ar) * 11 > len) {
        return "count-oob";
    }
    put_s(&o, "ok id=");
    put_u64(&o, id);
    put_s(&o, " flags=");
    put_u64(&o, flags);
    put_s(&o, " qd=");
    put_u64(&o, qd);
    put_s(&o, " an=");
    put_u64(&o, an);
    put_s(&o, " ns=");
    put_u64(&o, ns);
    put_s(&o, " ar=");
    put_u64(&o, ar);

    size_t off = 12;
    put_s(&o, " q=");
    for (unsigned i = 0; i < qd; i++) {
        if (i > 0) {
            put_c(&o, ',');
        }
        size_t next = off;
        const char *err = read_name(d, len, off, &o, &next);
        if (err != NULL) {
            return err;
        }
        off = next;
        if (off + 4 > len) {
            return "truncated";
        }
        put_c(&o, ':');
        put_u64(&o, (unsigned)((d[off] << 8) | d[off + 1]));
        put_c(&o, ':');
        put_u64(&o, (unsigned)((d[off + 2] << 8) | d[off + 3]));
        off += 4;
    }

    put_s(&o, " rr=");
    unsigned counts[3] = { an, ns, ar };
    unsigned rec = 0;
    for (unsigned sec = 0; sec < 3; sec++) {
        for (unsigned i = 0; i < counts[sec]; i++) {
            if (rec > 0) {
                put_c(&o, ',');
            }
            rec++;
            put_s(&o, section_name(sec));
            put_c(&o, ':');
            size_t next = off;
            const char *err = read_name(d, len, off, &o, &next);
            if (err != NULL) {
                return err;
            }
            off = next;
            if (off + 10 > len) {
                return "truncated";
            }
            unsigned type = (unsigned)((d[off] << 8) | d[off + 1]);
            unsigned clas = (unsigned)((d[off + 2] << 8) | d[off + 3]);
            unsigned long long ttl = ((unsigned long long)d[off + 4] << 24) |
                                     ((unsigned long long)d[off + 5] << 16) |
                                     ((unsigned long long)d[off + 6] << 8) |
                                     (unsigned long long)d[off + 7];
            size_t rdlen = (size_t)((d[off + 8] << 8) | d[off + 9]);
            off += 10;
            if (off + rdlen > len) {
                return "rdata-oob";
            }
            put_c(&o, ':');
            put_u64(&o, type);
            put_c(&o, ':');
            put_u64(&o, clas);
            put_c(&o, ':');
            put_u64(&o, ttl);
            put_c(&o, ':');
            // A and AAAA are parsed into an address and the class must be IN;
            // every other type, EDNS0 OPT (41) included, is skipped by RDLENGTH
            // and reported as its type number with no attempt to read its RDATA.
            if (type == 1 || type == 28) {
                if (clas != 1) {
                    return "class-not-in";
                }
                if (type == 1 && rdlen != 4) {
                    return "rdlength-mismatch";
                }
                if (type == 28 && rdlen != 16) {
                    return "rdlength-mismatch";
                }
                put_address(&o, d, off, type);
            } else {
                put_c(&o, '-');
            }
            off += rdlen;
        }
    }
    // Trailing bytes are allowed: they are not part of the message this parser
    // models, and a real reply can carry padding.
    return NULL;
}

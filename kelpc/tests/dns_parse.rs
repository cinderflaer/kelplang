//! The differential corpus for the DNS message parser (MK7 §7 item 6, §8).
//!
//! §8's contract is "build each runtime piece in Kelp first, keep the C as the
//! reference, and prove it with a differential corpus". This is that, for the
//! message-parsing half of the stub resolver, and §7 names the resolver as the
//! item most likely to be deferred and the hardest to get right. The tidy path
//! (a well-formed A reply) is not why this exists: name compression, a pointer
//! that loops or leaves the buffer, a truncated message and a count that cannot
//! fit are.
//!
//! The two implementations:
//!
//!   * `kelpc/runtime/dns_parse_ref.c` — the C reference. Nothing includes it
//!     but this test, which compiles it directly.
//!   * `kelpc/runtime/dns_parse.kp` — the same parser in Kelp, compiled by the
//!     C backend. It is not a std module and is not shipped yet.
//!
//! Both are fed the same corpus and each prints one line per case; the test
//! compares the two line streams exactly (same count, same text, same order).
//! Every case contributes exactly one line, so a mismatch is the case at that
//! index.
//!
//! # The covered contract
//!
//! * 12-byte header: id, flags, qdcount, ancount, nscount, arcount. A message
//!   shorter than the header is `short-header`.
//! * Question section: QNAME labels, QTYPE, QCLASS.
//! * Resource records in the answer, authority and additional sections: NAME
//!   (with compression pointers), TYPE, CLASS, TTL, RDLENGTH, RDATA.
//! * Name compression: a pointer is two octets with the top two bits set and
//!   must point strictly backwards into the message; a self or forward pointer
//!   (the only way a loop can be written) is `pointer-backward`, and a pointer
//!   past the end is `pointer-oob`. Neither is followed, so neither can walk
//!   forever. A label length octet with either reserved top bit set is
//!   `bad-label`. A name whose decompressed length would exceed 255 octets,
//!   terminator included, is `name-too-long`; the corpus holds the accepting
//!   side of that bound (255 octets exactly) as well.
//! * A (type 1) and AAAA (type 28) RDATA is parsed into an address; the class
//!   must be IN (1) or the record is `class-not-in`, and RDLENGTH must be 4 or
//!   16 or it is `rdlength-mismatch`. A record of any other type, EDNS0 OPT
//!   (type 41) included, is skipped by RDLENGTH and reported as its type
//!   number, with no attempt to parse its RDATA.
//! * Truncated input is `truncated`; RDATA running past the end is `rdata-oob`;
//!   a count the message cannot hold is `count-oob`, refused before any entry
//!   is read. None of these is a panic, and none leaves partial output: a
//!   rejected case prints exactly one `reject <reason>` line.
//! * Trailing bytes after the last record are allowed and ignored.
//!
//! # Not covered, and why
//!
//! Transport is a different lane and this parser never sees it: UDP and TCP,
//! `/etc/hosts`, `/etc/resolv.conf`, retries, timeouts, TCP fallback and search
//! domains are the resolver, not the parser. The parser is bytes in, a parsed
//! result out, with no sockets and no files.
//!
//! # The corpus
//!
//! Hand-written cases for every rule above; recorded real responses kept as
//! their exact wire bytes with the query that produced them; and a deterministic
//! pseudo-random fan (systematic single-byte flips and truncations of the valid
//! messages, then seeded random edits) because a parser that agrees on the tidy
//! cases is exactly the parser that comes apart on the others. The test asserts
//! both verdicts occur and that every rejection reason above is exercised, so a
//! corpus that stopped deciding one way, or stopped covering a rejection class,
//! fails here rather than passing quietly.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every reason the parser can print. The corpus must exercise all of them, or
/// it is not covering the contract it claims to.
const REASONS: [&str; 11] = [
    "short-header",
    "count-oob",
    "truncated",
    "bad-label",
    "label-oob",
    "pointer-oob",
    "pointer-backward",
    "name-too-long",
    "class-not-in",
    "rdlength-mismatch",
    "rdata-oob",
];

struct Case {
    what: &'static str,
    bytes: Vec<u8>,
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

// ---- message builders -------------------------------------------------------

fn header(id: u16, flags: u16, qd: u16, an: u16, ns: u16, ar: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(12);
    for f in [id, flags, qd, an, ns, ar] {
        v.extend_from_slice(&f.to_be_bytes());
    }
    v
}

/// A name in wire form. The empty string is the root (a single zero octet).
fn dname(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    if !s.is_empty() {
        for label in s.split('.') {
            v.push(label.len() as u8);
            v.extend_from_slice(label.as_bytes());
        }
    }
    v.push(0);
    v
}

/// A compression pointer to `off`.
fn pointer(off: usize) -> Vec<u8> {
    vec![0xC0 | ((off >> 8) as u8 & 0x3F), (off & 0xFF) as u8]
}

fn question(name: &[u8], qtype: u16, qclass: u16) -> Vec<u8> {
    let mut v = name.to_vec();
    v.extend_from_slice(&qtype.to_be_bytes());
    v.extend_from_slice(&qclass.to_be_bytes());
    v
}

fn rr(name: &[u8], rtype: u16, rclass: u16, ttl: u32, rdata: &[u8]) -> Vec<u8> {
    let mut v = name.to_vec();
    v.extend_from_slice(&rtype.to_be_bytes());
    v.extend_from_slice(&rclass.to_be_bytes());
    v.extend_from_slice(&ttl.to_be_bytes());
    v.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    v.extend_from_slice(rdata);
    v
}

/// A question for `example.com`, so the answer can compress to offset 12.
fn example_question(qtype: u16) -> Vec<u8> {
    question(&dname("example.com"), qtype, 1)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    (0..bytes.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

// ---- the corpus -------------------------------------------------------------

fn corpus() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    // The messages a fan is built from: every well-formed one, so a truncation
    // or a flip is a small edit of something that parsed.
    let mut valid: Vec<Vec<u8>> = Vec::new();
    let accept = |cases: &mut Vec<Case>,
                      valid: &mut Vec<Vec<u8>>,
                      what: &'static str,
                      bytes: Vec<u8>,
                      expect: Option<&'static str>| {
        valid.push(bytes.clone());
        cases.push(Case { what, bytes, expect });
    };
    let reject = |cases: &mut Vec<Case>,
                      what: &'static str,
                      bytes: Vec<u8>,
                      expect: Option<&'static str>| {
        cases.push(Case { what, bytes, expect });
    };

    // ---- hand-written: the accepting side ---------------------------------

    // A well-formed A answer. The answer's name is a compression pointer to the
    // question's name at offset 12 (`c00c`), which is the ordinary shape.
    {
        let mut m = header(0x1234, 0x8180, 1, 1, 0, 0);
        m.extend(example_question(1));
        m.extend(rr(&pointer(12), 1, 1, 300, &[93, 184, 216, 34]));
        accept(
            &mut cases,
            &mut valid,
            "A answer, answer name compressed to the question",
            m,
            Some("ok id=4660 flags=33152 qd=1 an=1 ns=0 ar=0 q=6578616d706c65.636f6d:1:1 rr=an:6578616d706c65.636f6d:1:1:300:93.184.216.34"),
        );
    }

    // AAAA, so the other address form is parsed and rendered.
    {
        let mut m = header(0x2345, 0x8180, 1, 1, 0, 0);
        m.extend(example_question(28));
        m.extend(rr(
            &pointer(12),
            28,
            1,
            60,
            &[0x26, 0x06, 0x47, 0x00, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        ));
        accept(
            &mut cases,
            &mut valid,
            "AAAA answer",
            m,
            Some("ok id=9029 flags=33152 qd=1 an=1 ns=0 ar=0 q=6578616d706c65.636f6d:28:1 rr=an:6578616d706c65.636f6d:28:1:60:2606:4700:0010:0000:0000:0000:0000:0001"),
        );
    }

    // A pointer chain: the second record's name is a pointer to the first
    // record's name, which is itself a pointer to the question.
    {
        let mut m = header(0x0001, 0x8180, 1, 2, 0, 0);
        m.extend(example_question(1)); // 12..29
        m.extend(rr(&pointer(12), 1, 1, 300, &[1, 2, 3, 4])); // 29..45
        m.extend(rr(&pointer(29), 1, 1, 300, &[5, 6, 7, 8])); // 45..61
        accept(
            &mut cases,
            &mut valid,
            "a pointer to a name that itself ends in a pointer",
            m,
            Some("ok id=1 flags=33152 qd=1 an=2 ns=0 ar=0 q=6578616d706c65.636f6d:1:1 rr=an:6578616d706c65.636f6d:1:1:300:1.2.3.4,an:6578616d706c65.636f6d:1:1:300:5.6.7.8"),
        );
    }

    // All four counts nonzero: an answer, an authority NS (type 2, skipped by
    // RDLENGTH) and an additional glue A record.
    {
        let mut m = header(0x4242, 0x8180, 1, 1, 1, 1);
        m.extend(example_question(1));
        m.extend(rr(&pointer(12), 1, 1, 300, &[203, 0, 113, 7]));
        m.extend(rr(&pointer(12), 2, 1, 3600, &dname("ns1.example.com")));
        m.extend(rr(&dname("ns1.example.com"), 1, 1, 300, &[203, 0, 113, 8]));
        accept(
            &mut cases,
            &mut valid,
            "all four counts nonzero",
            m,
            Some("ok id=16962 flags=33152 qd=1 an=1 ns=1 ar=1 q=6578616d706c65.636f6d:1:1 rr=an:6578616d706c65.636f6d:1:1:300:203.0.113.7,ns:6578616d706c65.636f6d:2:1:3600:-,ar:6e7331.6578616d706c65.636f6d:1:1:300:203.0.113.8"),
        );
    }

    // A name of exactly 255 octets decompressed is the accepting side of the
    // bound: four labels whose length octets and data total 254 bytes, plus the
    // terminator.
    {
        // 63 + 63 + 63 + 61 label bytes, and four length octets, and the zero.
        let mut name = Vec::new();
        for len in [63usize, 63, 63, 61] {
            name.push(len as u8);
            name.extend(vec![b'a'; len]);
        }
        name.push(0);
        let mut m = header(0x0055, 0x8180, 1, 0, 0, 0);
        m.extend(question(&name, 1, 1));
        accept(
            &mut cases,
            &mut valid,
            "a 255-octet decompressed name is accepted",
            m,
            Some("ok"),
        );
    }

    // An unknown type is reported by number and its RDATA is never read, even
    // when the RDATA is structured (an MX here is a preference and a name, and
    // none of it is parsed).
    {
        let mut m = header(0x0077, 0x8180, 1, 1, 0, 0);
        m.extend(example_question(15));
        m.extend(rr(&pointer(12), 15, 1, 60, &[0, 10, 4, b'm', b'a', b'i', b'l']));
        accept(
            &mut cases,
            &mut valid,
            "an MX record is skipped and reported by type number",
            m,
            Some("ok id=119 flags=33152 qd=1 an=1 ns=0 ar=0 q=6578616d706c65.636f6d:15:1 rr=an:6578616d706c65.636f6d:15:1:60:-"),
        );
    }

    // EDNS0 OPT (type 41) additional, skipped like any other unknown type. Its
    // owner name is the root, which renders as `-`.
    {
        let mut m = header(0x0007, 0x8180, 1, 0, 0, 1);
        m.extend(example_question(1));
        m.extend(rr(&dname(""), 41, 4096, 0, &[]));
        accept(
            &mut cases,
            &mut valid,
            "EDNS0 OPT is skipped like any unknown type",
            m,
            Some("ok id=7 flags=33152 qd=1 an=0 ns=0 ar=1 q=6578616d706c65.636f6d:1:1 rr=ar:-:41:4096:0:-"),
        );
    }

    // Trailing bytes after the last record are ignored, not a rejection.
    {
        let mut m = header(0x0008, 0x8180, 1, 0, 0, 0);
        m.extend(example_question(1));
        m.extend([0xde, 0xad, 0xbe, 0xef]);
        accept(
            &mut cases,
            &mut valid,
            "trailing bytes are ignored",
            m,
            Some("ok"),
        );
    }

    // ---- hand-written: the rejecting side ---------------------------------

    reject(
        &mut cases,
        "empty input",
        Vec::new(),
        Some("reject short-header"),
    );
    reject(
        &mut cases,
        "an 11-byte message is shorter than the header",
        vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0],
        Some("reject short-header"),
    );

    // A count the message cannot hold, refused before any entry is read.
    reject(
        &mut cases,
        "qdcount 5 in a 12-byte message",
        header(1, 0x8180, 5, 0, 0, 0),
        Some("reject count-oob"),
    );
    reject(
        &mut cases,
        "ancount 1 with no room for a record",
        header(1, 0x8180, 0, 1, 0, 0),
        Some("reject count-oob"),
    );
    reject(
        &mut cases,
        "arcount 65535",
        header(1, 0x8180, 0, 0, 0, 65535),
        Some("reject count-oob"),
    );

    // A fixed field cut off mid-read: the counts fit, the record does not.
    {
        let mut m = header(0x0001, 0x8180, 1, 1, 0, 0);
        m.extend(question(&dname("a"), 1, 1)); // 12..18
        m.extend([0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c, 0x00]); // 18..28
        reject(
            &mut cases,
            "a record's fixed fields run past the end",
            m,
            Some("reject truncated"),
        );
    }

    // A label length octet with the reserved bits set.
    {
        let mut m = header(0x0001, 0x8180, 1, 0, 0, 0);
        m.extend([0x40, 0x00, 0x01, 0x00, 0x01]);
        reject(
            &mut cases,
            "a label length octet with the reserved top bits set",
            m,
            Some("reject bad-label"),
        );
    }

    // A label that claims more bytes than remain.
    {
        let mut m = header(0x0001, 0x8180, 1, 0, 0, 0);
        m.extend([0x20, 0x00, 0x01, 0x00, 0x01]);
        reject(
            &mut cases,
            "a label runs past the end",
            m,
            Some("reject label-oob"),
        );
    }

    // A pointer past the end of the message.
    {
        let mut m = header(0x0001, 0x8180, 1, 0, 0, 0);
        m.extend([0xC0, 0xFF, 0x00, 0x01, 0x00]);
        reject(
            &mut cases,
            "a pointer past the end",
            m,
            Some("reject pointer-oob"),
        );
    }

    // A pointer to itself: the loop the strict-backwards rule exists to stop.
    {
        let mut m = header(0x0001, 0x8180, 1, 0, 0, 0);
        m.extend([0xC0, 0x0C, 0x00, 0x01, 0x00]);
        reject(
            &mut cases,
            "a self pointer (a loop)",
            m,
            Some("reject pointer-backward"),
        );
    }

    // A pointer forward, into the message but not backwards.
    {
        let mut m = header(0x0001, 0x8180, 1, 0, 0, 0);
        m.extend([0xC0, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        reject(
            &mut cases,
            "a forward pointer",
            m,
            Some("reject pointer-backward"),
        );
    }

    // Four 63-byte labels: 256 octets of length octets and label data, so the
    // name would be 257 octets, over the 255-octet bound.
    {
        let mut name = Vec::new();
        for _ in 0..4 {
            name.push(63u8);
            name.extend(vec![b'a'; 63]);
        }
        name.push(0);
        let mut m = header(0x0001, 0x8180, 1, 0, 0, 0);
        m.extend(question(&name, 1, 1));
        reject(
            &mut cases,
            "a 256-octet decompressed name",
            m,
            Some("reject name-too-long"),
        );
    }

    // An A record whose class is not IN.
    {
        let mut m = header(0x0001, 0x8180, 1, 1, 0, 0);
        m.extend(example_question(1));
        m.extend(rr(&pointer(12), 1, 3, 300, &[1, 2, 3, 4]));
        reject(
            &mut cases,
            "an A record with class CH",
            m,
            Some("reject class-not-in"),
        );
    }

    // A with RDLENGTH 5, and AAAA with RDLENGTH 4.
    {
        let mut m = header(0x0001, 0x8180, 1, 1, 0, 0);
        m.extend(example_question(1));
        m.extend(rr(&pointer(12), 1, 1, 300, &[1, 2, 3, 4, 5]));
        reject(
            &mut cases,
            "an A record with RDLENGTH 5",
            m,
            Some("reject rdlength-mismatch"),
        );
    }
    {
        let mut m = header(0x0001, 0x8180, 1, 1, 0, 0);
        m.extend(example_question(28));
        m.extend(rr(&pointer(12), 28, 1, 60, &[1, 2, 3, 4]));
        reject(
            &mut cases,
            "an AAAA record with RDLENGTH 4",
            m,
            Some("reject rdlength-mismatch"),
        );
    }

    // RDATA that runs past the end, on an unknown type so the length rule is
    // not what catches it.
    {
        let mut m = header(0x0001, 0x8180, 1, 1, 0, 0);
        m.extend(example_question(1));
        m.extend(rr(&pointer(12), 99, 1, 300, &[]));
        let n = m.len();
        m[n - 2] = 0x00; // RDLENGTH
        m[n - 1] = 0x32; // 50, but only three octets follow
        m.extend([0xaa, 0xbb, 0xcc]);
        reject(
            &mut cases,
            "RDATA that runs past the end",
            m,
            Some("reject rdata-oob"),
        );
    }

    // ---- recorded real responses ------------------------------------------
    //
    // Captured 2026-09-20 by sending the query below from this machine to
    // 1.1.1.1 over UDP and keeping the reply bytes verbatim. The query and the
    // reply are both exact.

    // Query: A example.com, RD.
    //   123401000001000000000000076578616d706c6503636f6d0000010001
    // Reply: two A records, both names a pointer to the question.
    accept(
        &mut cases,
        &mut valid,
        "recorded: A example.com from 1.1.1.1",
        from_hex("123481800001000200000000076578616d706c6503636f6d0000010001c00c000100010000005200046814179ac00c00010001000000520004ac4293f3"),
        None,
    );

    // Query: AAAA example.com, RD, with an EDNS0 OPT in additional (the reply
    // carries the OPT back, and arcount is 1).
    //   123401000001000000000001076578616d706c6503636f6d00001c00010000291000000000000000
    accept(
        &mut cases,
        &mut valid,
        "recorded: AAAA example.com from 1.1.1.1, with EDNS0",
        from_hex("123481800001000200000001076578616d706c6503636f6d00001c0001c00c001c00010000003a00102606470000100000000000006814179ac00c001c00010000003a0010260647000010000000000000ac4293f300002904d0000000000000"),
        None,
    );

    // Query: NS com, non-recursive (flags 0).
    //   12340000000100000000000003636f6d0000020001
    // Reply: thirteen NS records whose owner names compress to the question and
    // whose RDATA names compress to the first owner name.
    accept(
        &mut cases,
        &mut valid,
        "recorded: NS com non-recursive from 1.1.1.1",
        from_hex("123480800001000d0000000003636f6d0000020001c00c0002000100025934001401610c67746c642d73657276657273036e657400c00c000200010002593400040162c023c00c000200010002593400040163c023c00c000200010002593400040164c023c00c000200010002593400040165c023c00c000200010002593400040166c023c00c000200010002593400040167c023c00c000200010002593400040168c023c00c000200010002593400040169c023c00c00020001000259340004016ac023c00c00020001000259340004016bc023c00c00020001000259340004016cc023c00c00020001000259340004016dc023"),
        None,
    );

    // Query: A nonexistent-abc-123.example.com, RD.
    //   123401000001000000000000136e6f6e6578697374656e742d6162632d313233076578616d706c6503636f6d0000010001
    // Reply: NXDOMAIN with an SOA in the authority section (type 6, skipped by
    // RDLENGTH).
    accept(
        &mut cases,
        &mut valid,
        "recorded: NXDOMAIN with an SOA authority from 1.1.1.1",
        from_hex("123481800001000000010000136e6f6e6578697374656e742d6162632d313233076578616d706c6503636f6d0000010001c0200006000100000708003207656c6c696f7474026e730a636c6f7564666c617265c02803646e73c0488ff5549e000027100000096000093a8000000708"),
        None,
    );

    // ---- the deterministic fan --------------------------------------------
    //
    // An LCG, not a random source: the corpus has to be the same corpus on every
    // run and every machine, or a failure is not reproducible from the report.
    let mut seed: u64 = 0xb529_7a4d_8c68_1f5b;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };

    // Systematic truncations: every prefix of the first few valid messages. A
    // truncated message is the single most likely thing a hostile or broken
    // resolver sends, and every cut point is a different read at the edge.
    for start in 0..valid.len().min(4) {
        let base = valid[start].clone();
        for cut in 0..=base.len() {
            cases.push(Case {
                what: "truncation",
                bytes: base[..cut].to_vec(),
                expect: None,
            });
        }
    }

    // Systematic single-byte edits of one representative message: every
    // position, flipped high bit and low bit. This is where a length or a count
    // becomes one off, and where a name turns into a pointer or a bad label.
    {
        let base = valid[0].clone();
        for i in 0..base.len() {
            for mask in [0x01u8, 0x80] {
                let mut m = base.clone();
                m[i] ^= mask;
                cases.push(Case {
                    what: "single-byte flip",
                    bytes: m,
                    expect: None,
                });
            }
        }
    }

    // Random truncations and multi-byte flips of the valid messages.
    for _ in 0..1200 {
        let base = &valid[next() % valid.len()];
        if next() % 2 == 0 {
            let cut = next() % (base.len() + 1);
            cases.push(Case {
                what: "random truncation",
                bytes: base[..cut].to_vec(),
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
                bytes: m,
                expect: None,
            });
        }
    }

    // Random bytes from scratch, which lands on the header and count checks
    // more often than anything else.
    for _ in 0..400 {
        let len = next() % 48;
        let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        cases.push(Case {
            what: "random bytes",
            bytes,
            expect: None,
        });
    }

    cases
}

// ---- the two drivers --------------------------------------------------------

fn c_driver(cases: &[Case]) -> String {
    let mut out = String::new();
    out.push_str("/* Generated by kelpc/tests/dns_parse.rs: one line per case. */\n");
    out.push_str(&format!(
        "#include \"{}\"\n",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime/dns_parse_ref.c")
            .display()
    ));
    out.push_str("#include <stdio.h>\n#include <stddef.h>\n\nint main(void) {\n");
    out.push_str("    char line[1 << 20];\n");
    for c in cases {
        out.push_str("    {\n");
        if c.bytes.is_empty() {
            out.push_str("        static const unsigned char data[1] = { 0 };\n");
        } else {
            let mut bytes = String::new();
            for (i, b) in c.bytes.iter().enumerate() {
                if i % 16 == 0 {
                    bytes.push_str("\n            ");
                }
                bytes.push_str(&format!("0x{b:02x}, "));
            }
            out.push_str(&format!("        static const unsigned char data[] = {{{bytes}\n        }};\n"));
        }
        out.push_str(&format!("        static const size_t data_len = {};\n", c.bytes.len()));
        out.push_str("        const char *err = kelp_dns_parse(data, data_len, line, sizeof line);\n");
        out.push_str("        if (err != NULL) printf(\"reject %s\\n\", err);\n");
        out.push_str("        else printf(\"%s\\n\", line);\n");
        out.push_str("    }\n");
    }
    out.push_str("    return 0;\n}\n");
    out
}

/// The Kelp driver. A Kelp string literal cannot carry an arbitrary byte (no
/// `\xNN` escape), so each case passes its hex and the driver decodes it: the
/// same bytes reach both implementations.
fn kelp_driver(cases: &[Case]) -> String {
    let mut out = String::new();
    out.push_str("// Generated by kelpc/tests/dns_parse.rs: one line per case.\n");
    out.push_str("impl \"dns_parse\"\nimpl \"std/str\"\n\n");
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
        out.push_str(&format!(
            "    print(dns_parse.line(hex_to_bytes(\"{}\")))\n",
            hex(&c.bytes)
        ));
    }
    out.push_str("}\n");
    out
}

fn build_c_driver(cases: &[Case]) -> PathBuf {
    let src = tmp("dns_parse_c.c");
    let bin = tmp("dns_parse_c");
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
/// reason `cap_matcher.rs` pins it: this test is about the parser, not about
/// which backend is under test.
///
/// The generated driver is written into a directory of its own with a copy of
/// `dns_parse.kp`, because `impl "..."` resolves against the entry file's
/// directory and the module is deliberately not a std module.
fn build_kelp_driver(cases: &[Case]) -> PathBuf {
    let dir = tmp("dns_parse_kelp");
    std::fs::create_dir_all(&dir).expect("creating the Kelp driver directory");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/dns_parse.kp"),
        dir.join("dns_parse.kp"),
    )
    .expect("copying dns_parse.kp next to the driver");
    let src = dir.join("driver.kp");
    let bin = dir.join("dns_parse_kelp");
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
fn the_kelp_dns_parser_agrees_with_the_c_reference_on_the_whole_corpus() {
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
    // being exercised would quietly fall out of the contract.
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut seen: Vec<&str> = Vec::new();
    for line in &oracle {
        if line.starts_with("ok ") {
            accepted += 1;
        } else if let Some(reason) = line.strip_prefix("reject ") {
            rejected += 1;
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
        "dns_parse: {} cases, {accepted} accepted, {rejected} rejected, {} reasons exercised",
        cases.len(),
        seen.len()
    );

    for (i, (case, (want, got))) in cases.iter().zip(oracle.iter().zip(&port)).enumerate() {
        assert_eq!(
            want, got,
            "case {i} ({}) disagrees:\n  C    {want}\n  Kelp {got}",
            case.what
        );
    }
}

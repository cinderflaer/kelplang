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
    Outdated {
        id: "std-add-field",
        what: "`.add_field(`",
        instead: "write `.addField(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-add-file",
        what: "`.add_file(`",
        instead: "write `.addFile(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-add-header",
        what: "`.add_header(`",
        instead: "write `.addHeader(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-alloc-zeroed",
        what: "`.alloc_zeroed(`",
        instead: "write `.allocZeroed(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-and-then",
        what: "`.and_then(`",
        instead: "write `.andThen(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-arg-count",
        what: "`.arg_count(`",
        instead: "write `.argCount(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-bad-request",
        what: "`.bad_request(`",
        instead: "write `.badRequest(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-base32-decode",
        what: "`.base32_decode(`",
        instead: "write `.base32Decode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-base32-encode",
        what: "`.base32_encode(`",
        instead: "write `.base32Encode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-base58-decode",
        what: "`.base58_decode(`",
        instead: "write `.base58Decode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-base58-encode",
        what: "`.base58_encode(`",
        instead: "write `.base58Encode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-base64-decode",
        what: "`.base64_decode(`",
        instead: "write `.base64Decode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-base64-encode",
        what: "`.base64_encode(`",
        instead: "write `.base64Encode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-buf-reader",
        what: "`.buf_reader(`",
        instead: "write `.bufReader(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-byte-index",
        what: "`.byte_index(`",
        instead: "write `.byteIndex(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-ca-pem",
        what: "`.ca_pem(`",
        instead: "write `.caPem(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-char-at",
        what: "`.char_at(`",
        instead: "write `.charAt(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-char-count",
        what: "`.char_count(`",
        instead: "write `.charCount(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-client-cert-pem",
        what: "`.client_cert_pem(`",
        instead: "write `.clientCertPem(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-client-options",
        what: "`.client_options(`",
        instead: "write `.clientOptions(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-close-all",
        what: "`.close_all(`",
        instead: "write `.closeAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-close-idle",
        what: "`.close_idle(`",
        instead: "write `.closeIdle(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-collect-list",
        what: "`.collect_list(`",
        instead: "write `.collectList(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-commas-i64",
        what: "`.commas_i64(`",
        instead: "write `.commasI64(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-commas-u64",
        what: "`.commas_u64(`",
        instead: "write `.commasU64(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-connect-scheme",
        what: "`.connect_scheme(`",
        instead: "write `.connectScheme(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-connect-url",
        what: "`.connect_url(`",
        instead: "write `.connectUrl(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-content-type",
        what: "`.content_type(`",
        instead: "write `.contentType(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-copy-into",
        what: "`.copy_into(`",
        instead: "write `.copyInto(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-copy-range",
        what: "`.copy_range(`",
        instead: "write `.copyRange(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-current-id",
        what: "`.current_id(`",
        instead: "write `.currentId(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-decode-at",
        what: "`.decode_at(`",
        instead: "write `.decodeAt(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-ends-with",
        what: "`.ends_with(`",
        instead: "write `.endsWith(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-env-var",
        what: "`.env_var(`",
        instead: "write `.envVar(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-eprint-bare",
        what: "`.eprint_bare(`",
        instead: "write `.eprintBare(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-errno-text",
        what: "`.errno_text(`",
        instead: "write `.errnoText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-error-text",
        what: "`.error_text(`",
        instead: "write `.errorText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-expect-err",
        what: "`.expect_err(`",
        instead: "write `.expectErr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-file-name",
        what: "`.file_name(`",
        instead: "write `.fileName(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-find-all",
        what: "`.find_all(`",
        instead: "write `.findAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-find-at",
        what: "`.find_at(`",
        instead: "write `.findAt(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-find-index",
        what: "`.find_index(`",
        instead: "write `.findIndex(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-form-decode",
        what: "`.form_decode(`",
        instead: "write `.formDecode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-form-encode",
        what: "`.form_encode(`",
        instead: "write `.formEncode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-get-tls",
        what: "`.get_tls(`",
        instead: "write `.getTls(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-group-count",
        what: "`.group_count(`",
        instead: "write `.groupCount(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-group-digits",
        what: "`.group_digits(`",
        instead: "write `.groupDigits(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-handshake-opts",
        what: "`.handshake_opts(`",
        instead: "write `.handshakeOpts(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-handshake-timeout",
        what: "`.handshake_timeout(`",
        instead: "write `.handshakeTimeout(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-handshake-timeout-ms",
        what: "`.handshake_timeout_ms(`",
        instead: "write `.handshakeTimeoutMs(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-handshake-with",
        what: "`.handshake_with(`",
        instead: "write `.handshakeWith(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-has-extension",
        what: "`.has_extension(`",
        instead: "write `.hasExtension(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-has-header",
        what: "`.has_header(`",
        instead: "write `.hasHeader(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-hex-decode",
        what: "`.hex_decode(`",
        instead: "write `.hexDecode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-hex-encode",
        what: "`.hex_encode(`",
        instead: "write `.hexEncode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-hex-lower",
        what: "`.hex_lower(`",
        instead: "write `.hexLower(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-hex-upper",
        what: "`.hex_upper(`",
        instead: "write `.hexUpper(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-hex-value",
        what: "`.hex_value(`",
        instead: "write `.hexValue(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-home-dir",
        what: "`.home_dir(`",
        instead: "write `.homeDir(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-http-error-status",
        what: "`.http_error_status(`",
        instead: "write `.httpErrorStatus(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-http-error-text",
        what: "`.http_error_text(`",
        instead: "write `.httpErrorText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-human-scale",
        what: "`.human_scale(`",
        instead: "write `.humanScale(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-human-size",
        what: "`.human_size(`",
        instead: "write `.humanSize(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-index-of",
        what: "`.index_of(`",
        instead: "write `.indexOf(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-into-iter",
        what: "`.into_iter(`",
        instead: "write `.intoIter(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-absolute",
        what: "`.is_absolute(`",
        instead: "write `.isAbsolute(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-digit",
        what: "`.is_digit(`",
        instead: "write `.isDigit(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-dir",
        what: "`.is_dir(`",
        instead: "write `.isDir(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-err",
        what: "`.is_err(`",
        instead: "write `.isErr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-file",
        what: "`.is_file(`",
        instead: "write `.isFile(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-finite",
        what: "`.is_finite(`",
        instead: "write `.isFinite(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-match",
        what: "`.is_match(`",
        instead: "write `.isMatch(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-nan",
        what: "`.is_nan(`",
        instead: "write `.isNan(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-none",
        what: "`.is_none(`",
        instead: "write `.isNone(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-ok",
        what: "`.is_ok(`",
        instead: "write `.isOk(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-open",
        what: "`.is_open(`",
        instead: "write `.isOpen(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-running",
        what: "`.is_running(`",
        instead: "write `.isRunning(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-some",
        what: "`.is_some(`",
        instead: "write `.isSome(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-space",
        what: "`.is_space(`",
        instead: "write `.isSpace(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-symlink",
        what: "`.is_symlink(`",
        instead: "write `.isSymlink(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-is-tls",
        what: "`.is_tls(`",
        instead: "write `.isTls(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-join-all",
        what: "`.join_all(`",
        instead: "write `.joinAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-join-or-report",
        what: "`.join_or_report(`",
        instead: "write `.joinOrReport(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-key-password",
        what: "`.key_password(`",
        instead: "write `.keyPassword(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-last-sep",
        what: "`.last_sep(`",
        instead: "write `.lastSep(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-listen-on",
        what: "`.listen_on(`",
        instead: "write `.listenOn(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-local-parts",
        what: "`.local_parts(`",
        instead: "write `.localParts(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-make-get",
        what: "`.make_get(`",
        instead: "write `.makeGet(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-make-temp-dir",
        what: "`.make_temp_dir(`",
        instead: "write `.makeTempDir(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-make-temp-file",
        what: "`.make_temp_file(`",
        instead: "write `.makeTempFile(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-map-err",
        what: "`.map_err(`",
        instead: "write `.mapErr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-min-version",
        what: "`.min_version(`",
        instead: "write `.minVersion(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-mkdir-all",
        what: "`.mkdir_all(`",
        instead: "write `.mkdirAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-mode-text",
        what: "`.mode_text(`",
        instead: "write `.modeText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-mono-ms",
        what: "`.mono_ms(`",
        instead: "write `.monoMs(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-multipart-request",
        what: "`.multipart_request(`",
        instead: "write `.multipartRequest(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-not-found",
        what: "`.not_found(`",
        instead: "write `.notFound(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-now-mono-ns",
        what: "`.now_mono_ns(`",
        instead: "write `.nowMonoNs(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-now-unix",
        what: "`.now_unix(`",
        instead: "write `.nowUnix(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-offset-at",
        what: "`.offset_at(`",
        instead: "write `.offsetAt(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-ok-text",
        what: "`.ok_text(`",
        instead: "write `.okText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-or-else",
        what: "`.or_else(`",
        instead: "write `.orElse(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-pad-left",
        what: "`.pad_left(`",
        instead: "write `.padLeft(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-pad-right",
        what: "`.pad_right(`",
        instead: "write `.padRight(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-parse-err",
        what: "`.parse_err(`",
        instead: "write `.parseErr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-parse-f64",
        what: "`.parse_f64(`",
        instead: "write `.parseF64(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-parse-i64",
        what: "`.parse_i64(`",
        instead: "write `.parseI64(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-parse-u64",
        what: "`.parse_u64(`",
        instead: "write `.parseU64(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-parse-url",
        what: "`.parse_url(`",
        instead: "write `.parseUrl(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-path-segments",
        what: "`.path_segments(`",
        instead: "write `.pathSegments(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-peer-addr",
        what: "`.peer_addr(`",
        instead: "write `.peerAddr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-peer-cert-subject",
        what: "`.peer_cert_subject(`",
        instead: "write `.peerCertSubject(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-percent-decode",
        what: "`.percent_decode(`",
        instead: "write `.percentDecode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-percent-encode",
        what: "`.percent_encode(`",
        instead: "write `.percentEncode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-post-text",
        what: "`.post_text(`",
        instead: "write `.postText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-print-bare",
        what: "`.print_bare(`",
        instead: "write `.printBare(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-query-param",
        what: "`.query_param(`",
        instead: "write `.queryParam(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-range-step",
        what: "`.range_step(`",
        instead: "write `.rangeStep(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-rate-limit",
        what: "`.rate_limit(`",
        instead: "write `.rateLimit(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-all",
        what: "`.read_all(`",
        instead: "write `.readAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-frame",
        what: "`.read_frame(`",
        instead: "write `.readFrame(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-into",
        what: "`.read_into(`",
        instead: "write `.readInto(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-key",
        what: "`.read_key(`",
        instead: "write `.readKey(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-line",
        what: "`.read_line(`",
        instead: "write `.readLine(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-line-into",
        what: "`.read_line_into(`",
        instead: "write `.readLineInto(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-lines",
        what: "`.read_lines(`",
        instead: "write `.readLines(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-link",
        what: "`.read_link(`",
        instead: "write `.readLink(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-pem",
        what: "`.read_pem(`",
        instead: "write `.readPem(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-read-text",
        what: "`.read_text(`",
        instead: "write `.readText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-recv-into",
        what: "`.recv_into(`",
        instead: "write `.recvInto(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-recv-timeout",
        what: "`.recv_timeout(`",
        instead: "write `.recvTimeout(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-reload-pem",
        what: "`.reload_pem(`",
        instead: "write `.reloadPem(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-remove-all",
        what: "`.remove_all(`",
        instead: "write `.removeAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-replace-all",
        what: "`.replace_all(`",
        instead: "write `.replaceAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-require-client-cert-pem",
        what: "`.require_client_cert_pem(`",
        instead: "write `.requireClientCertPem(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-reset-steps",
        what: "`.reset_steps(`",
        instead: "write `.resetSteps(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-retry-after",
        what: "`.retry_after(`",
        instead: "write `.retryAfter(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-run-in",
        what: "`.run_in(`",
        instead: "write `.runIn(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-run-inherit",
        what: "`.run_inherit(`",
        instead: "write `.runInherit(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-run-inherit-in",
        what: "`.run_inherit_in(`",
        instead: "write `.runInheritIn(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-send-all",
        what: "`.send_all(`",
        instead: "write `.sendAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-send-binary",
        what: "`.send_binary(`",
        instead: "write `.sendBinary(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-send-text",
        what: "`.send_text(`",
        instead: "write `.sendText(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-serve-n",
        what: "`.serve_n(`",
        instead: "write `.serveN(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-server-config-pem",
        what: "`.server_config_pem(`",
        instead: "write `.serverConfigPem(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-server-config-pem-with",
        what: "`.server_config_pem_with(`",
        instead: "write `.serverConfigPemWith(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-server-options",
        what: "`.server_options(`",
        instead: "write `.serverOptions(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-serve-threaded",
        what: "`.serve_threaded(`",
        instead: "write `.serveThreaded(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-serve-tls",
        what: "`.serve_tls(`",
        instead: "write `.serveTls(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-set-accept-timeout",
        what: "`.set_accept_timeout(`",
        instead: "write `.setAcceptTimeout(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-set-env-var",
        what: "`.set_env_var(`",
        instead: "write `.setEnvVar(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-set-idle-timeout",
        what: "`.set_idle_timeout(`",
        instead: "write `.setIdleTimeout(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-set-mode",
        what: "`.set_mode(`",
        instead: "write `.setMode(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-set-nonblocking",
        what: "`.set_nonblocking(`",
        instead: "write `.setNonblocking(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-set-timeout",
        what: "`.set_timeout(`",
        instead: "write `.setTimeout(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-shutdown-write",
        what: "`.shutdown_write(`",
        instead: "write `.shutdownWrite(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-sleep-ms",
        what: "`.sleep_ms(`",
        instead: "write `.sleepMs(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-spawn-in",
        what: "`.spawn_in(`",
        instead: "write `.spawnIn(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-starts-with",
        what: "`.starts_with(`",
        instead: "write `.startsWith(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-starts-with-at",
        what: "`.starts_with_at(`",
        instead: "write `.startsWithAt(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-sum-i64",
        what: "`.sum_i64(`",
        instead: "write `.sumI64(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-temp-dir",
        what: "`.temp_dir(`",
        instead: "write `.tempDir(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-temp-path",
        what: "`.temp_path(`",
        instead: "write `.tempPath(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-to-client",
        what: "`.to_client(`",
        instead: "write `.toClient(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-to-lower",
        what: "`.to_lower(`",
        instead: "write `.toLower(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-to-unix",
        what: "`.to_unix(`",
        instead: "write `.toUnix(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-to-upper",
        what: "`.to_upper(`",
        instead: "write `.toUpper(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-trim-end",
        what: "`.trim_end(`",
        instead: "write `.trimEnd(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-trim-start",
        what: "`.trim_start(`",
        instead: "write `.trimStart(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-unset-env-var",
        what: "`.unset_env_var(`",
        instead: "write `.unsetEnvVar(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-unwrap-err",
        what: "`.unwrap_err(`",
        instead: "write `.unwrapErr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-unwrap-or",
        what: "`.unwrap_or(`",
        instead: "write `.unwrapOr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-user-name",
        what: "`.user_name(`",
        instead: "write `.userName(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-wait-readable",
        what: "`.wait_readable(`",
        instead: "write `.waitReadable(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-wait-writable",
        what: "`.wait_writable(`",
        instead: "write `.waitWritable(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-with-ca-pem",
        what: "`.with_ca_pem(`",
        instead: "write `.withCaPem(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-with-header",
        what: "`.with_header(`",
        instead: "write `.withHeader(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-with-timeout-ms",
        what: "`.with_timeout_ms(`",
        instead: "write `.withTimeoutMs(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-with-timeouts",
        what: "`.with_timeouts(`",
        instead: "write `.withTimeouts(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-write-all",
        what: "`.write_all(`",
        instead: "write `.writeAll(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-write-str",
        what: "`.write_str(`",
        instead: "write `.writeStr(`, which is the same function under its camelCase name (issue #375)",
    },
    Outdated {
        id: "std-write-text",
        what: "`.write_text(`",
        instead: "write `.writeText(`, which is the same function under its camelCase name (issue #375)",
    },
];

/// The snake_case names of the standard library that have a camelCase spelling (issue
/// #375, phase 1 of #368): each one still builds through a wrapper, and a call through
/// it prints the finding of its register row. The scan reads the table, so a name
/// is added here and in the register together.
pub const STD_RENAMES: &[(&str, &str)] = &[
    ("add_field", "std-add-field"),
    ("add_file", "std-add-file"),
    ("add_header", "std-add-header"),
    ("alloc_zeroed", "std-alloc-zeroed"),
    ("and_then", "std-and-then"),
    ("arg_count", "std-arg-count"),
    ("bad_request", "std-bad-request"),
    ("base32_decode", "std-base32-decode"),
    ("base32_encode", "std-base32-encode"),
    ("base58_decode", "std-base58-decode"),
    ("base58_encode", "std-base58-encode"),
    ("base64_decode", "std-base64-decode"),
    ("base64_encode", "std-base64-encode"),
    ("buf_reader", "std-buf-reader"),
    ("byte_index", "std-byte-index"),
    ("ca_pem", "std-ca-pem"),
    ("char_at", "std-char-at"),
    ("char_count", "std-char-count"),
    ("client_cert_pem", "std-client-cert-pem"),
    ("client_options", "std-client-options"),
    ("close_all", "std-close-all"),
    ("close_idle", "std-close-idle"),
    ("collect_list", "std-collect-list"),
    ("commas_i64", "std-commas-i64"),
    ("commas_u64", "std-commas-u64"),
    ("connect_scheme", "std-connect-scheme"),
    ("connect_url", "std-connect-url"),
    ("content_type", "std-content-type"),
    ("copy_into", "std-copy-into"),
    ("copy_range", "std-copy-range"),
    ("current_id", "std-current-id"),
    ("decode_at", "std-decode-at"),
    ("ends_with", "std-ends-with"),
    ("env_var", "std-env-var"),
    ("eprint_bare", "std-eprint-bare"),
    ("errno_text", "std-errno-text"),
    ("error_text", "std-error-text"),
    ("expect_err", "std-expect-err"),
    ("file_name", "std-file-name"),
    ("find_all", "std-find-all"),
    ("find_at", "std-find-at"),
    ("find_index", "std-find-index"),
    ("form_decode", "std-form-decode"),
    ("form_encode", "std-form-encode"),
    ("get_tls", "std-get-tls"),
    ("group_count", "std-group-count"),
    ("group_digits", "std-group-digits"),
    ("handshake_opts", "std-handshake-opts"),
    ("handshake_timeout", "std-handshake-timeout"),
    ("handshake_timeout_ms", "std-handshake-timeout-ms"),
    ("handshake_with", "std-handshake-with"),
    ("has_extension", "std-has-extension"),
    ("has_header", "std-has-header"),
    ("hex_decode", "std-hex-decode"),
    ("hex_encode", "std-hex-encode"),
    ("hex_lower", "std-hex-lower"),
    ("hex_upper", "std-hex-upper"),
    ("hex_value", "std-hex-value"),
    ("home_dir", "std-home-dir"),
    ("http_error_status", "std-http-error-status"),
    ("http_error_text", "std-http-error-text"),
    ("human_scale", "std-human-scale"),
    ("human_size", "std-human-size"),
    ("index_of", "std-index-of"),
    ("into_iter", "std-into-iter"),
    ("is_absolute", "std-is-absolute"),
    ("is_digit", "std-is-digit"),
    ("is_dir", "std-is-dir"),
    ("is_err", "std-is-err"),
    ("is_file", "std-is-file"),
    ("is_finite", "std-is-finite"),
    ("is_match", "std-is-match"),
    ("is_nan", "std-is-nan"),
    ("is_none", "std-is-none"),
    ("is_ok", "std-is-ok"),
    ("is_open", "std-is-open"),
    ("is_running", "std-is-running"),
    ("is_some", "std-is-some"),
    ("is_space", "std-is-space"),
    ("is_symlink", "std-is-symlink"),
    ("is_tls", "std-is-tls"),
    ("join_all", "std-join-all"),
    ("join_or_report", "std-join-or-report"),
    ("key_password", "std-key-password"),
    ("last_sep", "std-last-sep"),
    ("listen_on", "std-listen-on"),
    ("local_parts", "std-local-parts"),
    ("make_get", "std-make-get"),
    ("make_temp_dir", "std-make-temp-dir"),
    ("make_temp_file", "std-make-temp-file"),
    ("map_err", "std-map-err"),
    ("min_version", "std-min-version"),
    ("mkdir_all", "std-mkdir-all"),
    ("mode_text", "std-mode-text"),
    ("mono_ms", "std-mono-ms"),
    ("multipart_request", "std-multipart-request"),
    ("not_found", "std-not-found"),
    ("now_mono_ns", "std-now-mono-ns"),
    ("now_unix", "std-now-unix"),
    ("offset_at", "std-offset-at"),
    ("ok_text", "std-ok-text"),
    ("or_else", "std-or-else"),
    ("pad_left", "std-pad-left"),
    ("pad_right", "std-pad-right"),
    ("parse_err", "std-parse-err"),
    ("parse_f64", "std-parse-f64"),
    ("parse_i64", "std-parse-i64"),
    ("parse_u64", "std-parse-u64"),
    ("parse_url", "std-parse-url"),
    ("path_segments", "std-path-segments"),
    ("peer_addr", "std-peer-addr"),
    ("peer_cert_subject", "std-peer-cert-subject"),
    ("percent_decode", "std-percent-decode"),
    ("percent_encode", "std-percent-encode"),
    ("post_text", "std-post-text"),
    ("print_bare", "std-print-bare"),
    ("query_param", "std-query-param"),
    ("range_step", "std-range-step"),
    ("rate_limit", "std-rate-limit"),
    ("read_all", "std-read-all"),
    ("read_frame", "std-read-frame"),
    ("read_into", "std-read-into"),
    ("read_key", "std-read-key"),
    ("read_line", "std-read-line"),
    ("read_line_into", "std-read-line-into"),
    ("read_lines", "std-read-lines"),
    ("read_link", "std-read-link"),
    ("read_pem", "std-read-pem"),
    ("read_text", "std-read-text"),
    ("recv_into", "std-recv-into"),
    ("recv_timeout", "std-recv-timeout"),
    ("reload_pem", "std-reload-pem"),
    ("remove_all", "std-remove-all"),
    ("replace_all", "std-replace-all"),
    ("require_client_cert_pem", "std-require-client-cert-pem"),
    ("reset_steps", "std-reset-steps"),
    ("retry_after", "std-retry-after"),
    ("run_in", "std-run-in"),
    ("run_inherit", "std-run-inherit"),
    ("run_inherit_in", "std-run-inherit-in"),
    ("send_all", "std-send-all"),
    ("send_binary", "std-send-binary"),
    ("send_text", "std-send-text"),
    ("serve_n", "std-serve-n"),
    ("server_config_pem", "std-server-config-pem"),
    ("server_config_pem_with", "std-server-config-pem-with"),
    ("server_options", "std-server-options"),
    ("serve_threaded", "std-serve-threaded"),
    ("serve_tls", "std-serve-tls"),
    ("set_accept_timeout", "std-set-accept-timeout"),
    ("set_env_var", "std-set-env-var"),
    ("set_idle_timeout", "std-set-idle-timeout"),
    ("set_mode", "std-set-mode"),
    ("set_nonblocking", "std-set-nonblocking"),
    ("set_timeout", "std-set-timeout"),
    ("shutdown_write", "std-shutdown-write"),
    ("sleep_ms", "std-sleep-ms"),
    ("spawn_in", "std-spawn-in"),
    ("starts_with", "std-starts-with"),
    ("starts_with_at", "std-starts-with-at"),
    ("sum_i64", "std-sum-i64"),
    ("temp_dir", "std-temp-dir"),
    ("temp_path", "std-temp-path"),
    ("to_client", "std-to-client"),
    ("to_lower", "std-to-lower"),
    ("to_unix", "std-to-unix"),
    ("to_upper", "std-to-upper"),
    ("trim_end", "std-trim-end"),
    ("trim_start", "std-trim-start"),
    ("unset_env_var", "std-unset-env-var"),
    ("unwrap_err", "std-unwrap-err"),
    ("unwrap_or", "std-unwrap-or"),
    ("user_name", "std-user-name"),
    ("wait_readable", "std-wait-readable"),
    ("wait_writable", "std-wait-writable"),
    ("with_ca_pem", "std-with-ca-pem"),
    ("with_header", "std-with-header"),
    ("with_timeout_ms", "std-with-timeout-ms"),
    ("with_timeouts", "std-with-timeouts"),
    ("write_all", "std-write-all"),
    ("write_str", "std-write-str"),
    ("write_text", "std-write-text"),
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
    // The camelCase renames of the standard library (issue #375): `.old(` on any
    // receiver, read off the tokens. A declaration (`fun T.old(`, the wrapper
    // itself) is not a use, so it is left alone.
    for (index, window) in tokens.windows(3).enumerate() {
        let [dot, name, open] = window else { continue };
        if !matches!(dot.kind, TokenKind::Dot) || !matches!(open.kind, TokenKind::LParen) {
            continue;
        }
        let TokenKind::Ident(text) = &name.kind else { continue };
        let Some(&(_, id)) = STD_RENAMES.iter().find(|(old, _)| *old == text.as_str()) else {
            continue;
        };
        if is_declaration_head(tokens, index) {
            continue;
        }
        found.push(finding(id, name.span, module));
    }
    found
}

/// Whether the dot at `dot` sits in a declaration head (`fun Type.name(`, with
/// `mut` and generic arguments): the walk back over the receiver stops at `fun`.
fn is_declaration_head(tokens: &[Token], dot: usize) -> bool {
    let mut at = dot;
    while at > 0 {
        at -= 1;
        match tokens[at].kind {
            TokenKind::Fun => return true,
            TokenKind::Ident(_) | TokenKind::Lt | TokenKind::Gt | TokenKind::Comma | TokenKind::Mut => {}
            _ => return false,
        }
    }
    false
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
            // The camelCase renames (issue #375): a shipped module calls the camelCase
            // name, and only a wrapper's declaration line names the old one.
            for line in text.lines() {
                let t = line.trim_start();
                if ["fun ", "must_use fun ", "pub fun ", "unsafe fun ", "extern fun ", "//"]
                    .iter()
                    .any(|head| t.starts_with(head))
                {
                    continue;
                }
                for (old, _) in STD_RENAMES {
                    assert!(
                        !line.contains(&format!(".{old}(")),
                        "the shipped module '{id}' calls `.{old}(`, which is outdated syntax: {line}"
                    );
                }
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

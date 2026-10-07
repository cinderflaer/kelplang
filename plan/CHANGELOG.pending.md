# Pending changes

Scratch file for changes that are not released yet. Gitignored, never committed.

Add a line here when a user-visible change lands in `kelp`. **Do not touch
`wiki/changelog.md` until a version is released**: the changelog page carries
releases only.

When a version drops, write that release's entry in `wiki/changelog.md` from
this file plus the tag's commits and the Gitea release body, then wipe this file
back to this header. No need to ask first.

Keep each line short and factual: what changed, and why it matters to someone
writing Kelp. Group under the headings below; delete the headings that stay
empty on release.
- `std/http`: `http.multipart(boundary)` builds a multipart/form-data body (`add_field`, `add_file`, `finish`, `content_type`), and `http.multipart_request(url, form)` makes the POST.
- `must_use fun`: a call used as a statement with its result dropped is a compile error. `Sender.send` and the `fs.try_*` mutators carry it; `val _r = ...` is the explicit discard.
- `std/tls`: `ServerOptions.key_password(phrase)` loads an encrypted private key; `ServerConfig.reload_pem(cert, key, &opts)` swaps the certificate for new handshakes and keeps the old one when the new material is bad; `tlsfile.read_key(path)` refuses a key file that group or others can read.
- `std/net`, `std/http`: `net.listen_on(host, port)` and `http.listen_on(host, port)` bind one host, checked on `host:port` against the `net` grant.
- `std/http`: `Pool.set_idle_timeout(ms)` and `Pool.close_idle(after_ms)` close kept connections that have idled too long (no `time` grant needed).

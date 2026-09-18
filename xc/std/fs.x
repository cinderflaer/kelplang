// std/fs — the gated filesystem namespace (M7).
//
// The domain gate is compile-time: a module without the `fs` capability
// cannot *name* `fs.read` at all (resolution fails), and a module that does
// hold it gets its patterns compiled in as constant data, checked against
// every path argument at each call site before the syscall runs. The raw
// syscalls are the externs below; everything a caller sees is ordinary X.
//
// Paths are `/`-separated on every platform in this API (the plan's `path`
// section): translating to `\` belongs at the syscall boundary.

extern fun read_raw(path: &str).R: Bytes
extern fun write_raw(path: &str, data: Bytes)
extern fun write_text_raw(path: &str, text: String)
extern fun exists_raw(path: &str).R: bool
extern fun is_file_raw(path: &str).R: bool
extern fun is_dir_raw(path: &str).R: bool
extern fun size_raw(path: &str).R: u64
extern fun remove_raw(path: &str)
extern fun rename_raw(from: &str, to: &str)
extern fun mkdir_raw(path: &str)
extern fun append_raw(path: &str, data: Bytes)
extern fun sync_raw(path: &str)

// The whole file, as owned bytes.
// Panics: if the file cannot be read.
fun read(path: &str).R: Bytes = read_raw(path)

// The whole file, as owned text.
// Panics: if the file cannot be read, or is not valid utf-8.
fun read_text(path: &str).R: String = String.from_bytes(read_raw(path))

// Writing takes the bytes or the text by value: the data is handed over, not
// borrowed, since the call consumes nothing else of it.
// Panics: if the file cannot be written.
fun write(path: &str, data: Bytes) {
    write_raw(path, data)
}

// Panics: if the file cannot be written.
fun write_text(path: &str, text: String) {
    write_text_raw(path, text)
}

fun exists(path: &str).R: bool = exists_raw(path)
fun is_file(path: &str).R: bool = is_file_raw(path)
fun is_dir(path: &str).R: bool = is_dir_raw(path)

// Panics: if the file does not exist.
fun size(path: &str).R: u64 = size_raw(path)

// Panics: if the file cannot be removed.
fun remove(path: &str) {
    remove_raw(path)
}

// Both paths are checked against the grant — `rename` is the one call with
// two resource arguments.
// Panics: if either path is outside the granted capability, or the rename
// fails.
fun rename(from: &str, to: &str) {
    rename_raw(from, to)
}

// One directory. `fs.mkdir_all` needs to walk prefixes of its path and is
// deferred with the rest of the growable-path work.
// Panics: if the directory cannot be created.
fun mkdir(path: &str) {
    mkdir_raw(path)
}

// Appends `data` to the file, creating it if it does not exist. This is the
// streaming primitive: it needs no growable buffer, only a buffer's worth of
// data at a time, which is why it lands before the collections milestone that
// `fs.append` was pencilled in with.
// Panics: if the file cannot be opened or written.
fun append(path: &str, data: Bytes) {
    append_raw(path, data)
}

// Forces everything written to the file so far out to storage. Without this a
// timing is a timing of the page cache; with it, of the device.
// Panics: if the file cannot be opened or synced.
fun sync(path: &str) {
    sync_raw(path)
}

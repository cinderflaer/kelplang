//! The compiler cache for incremental compilation (roadmap A-14,
//! `plan/incremental-compilation.md`).
//!
//! A unit's object file is a pure function of the LLVM IR text the unit's
//! module lowers to and of the machine it is compiled for: the target triple,
//! the CPU and feature string, the relocation and code model, the optimization
//! level, the debug-info setting, and the compiler version. There is no
//! dependency tracking and no interface file, because the IR already carries
//! every decision that could change the object: if two units' IR is identical,
//! their objects are identical, so a hit cannot be wrong.
//!
//! The key is the SHA-256 of a canonical byte string built from those labelled
//! fields plus the IR text (`object_key`), and the entry is `<key>.o` inside
//! the cache directory. A write goes to a temporary file in that directory and
//! is renamed into place, so two builds sharing a cache never observe a partial
//! entry. A missing, unreadable, empty or visibly truncated entry is a miss; an
//! entry that survives the lookup but fails to link is deleted by the caller so
//! the next build recovers (see `codegen_llvm::compile_units`).
//!
//! The same directory holds generated `.kpi` front-end interface artifacts.
//! Their names are stable hashes of module identities, while their contents
//! carry the public API and dependency hashes a later front-end reuse slice
//! will validate before it skips a module.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::sha256::sha256_hex;

/// The cache directory's name, created next to a build's other output.
pub const CACHE_DIR_NAME: &str = ".kelpc-cache";

/// Turns lookups and stores off for one build (`KELPC_CACHE=off`).
pub const OFF_ENV: &str = "KELPC_CACHE";

/// Overrides the cache directory.
pub const DIR_ENV: &str = "KELPC_CACHE_DIR";

/// Prints the per-build hit/miss/store line when set to `1`.
pub const STATS_ENV: &str = "KELPC_CACHE_STATS";

/// The size cap in mebibytes (`KELPC_CACHE_MAX_MB`), 1024 by default.
pub const MAX_MB_ENV: &str = "KELPC_CACHE_MAX_MB";

const INTERFACE_EXT: &str = "kpi";

/// The default size cap: 1 GiB, large enough that a project of ordinary size
/// never evicts and small enough that a long-lived cache cannot fill a disk.
const DEFAULT_MAX_MB: f64 = 1024.0;

/// Makes a temporary entry's name unique within one process, so two builds in
/// one process (the test suite runs several) cannot race for the same path.
static ENTRY_TMP: AtomicU64 = AtomicU64::new(0);

/// The labelled fields a unit object is a pure function of, other than the IR
/// itself. The machine fields come from `llvm_ffi::TargetMachine::key_fields` so
/// a machine constructed for a triple, subtarget, relocation model and code
/// model describes itself in one place.
pub struct KeyFields<'a> {
    pub triple: &'a str,
    pub cpu: &'a str,
    pub features: &'a str,
    pub reloc: &'a str,
    pub code_model: &'a str,
    pub opt: &'a str,
    pub debug: &'a str,
    pub compiler: &'a str,
}

/// The cache key: the lowercase hex SHA-256 of the canonical byte string.
///
/// The fields are labelled and newline-separated so their identity is part of
/// the hashed bytes; the IR is last and length-prefixed, because it is the one
/// value that can contain a newline and a length makes the layout unambiguous
/// whatever it holds.
pub fn object_key(fields: &KeyFields, ir: &str) -> String {
    let canonical = format!(
        "kelpc-object-cache-1\n\
         triple={}\ncpu={}\nfeatures={}\nreloc={}\ncode_model={}\n\
         opt={}\ndebug={}\ncompiler={}\nir-bytes={}\n{}",
        fields.triple,
        fields.cpu,
        fields.features,
        fields.reloc,
        fields.code_model,
        fields.opt,
        fields.debug,
        fields.compiler,
        ir.len(),
        ir,
    );
    sha256_hex(canonical.as_bytes())
}

/// One build's counters: hits, misses and entries stored.
#[derive(Default)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    stored: AtomicU64,
}

impl CacheStats {
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
    pub fn stored(&self) -> u64 {
        self.stored.load(Ordering::Relaxed)
    }
}

/// One cache directory, for the duration of one build.
pub struct Cache {
    dir: PathBuf,
    enabled: bool,
    stats: CacheStats,
}

/// One entry the cache directory holds, as eviction sees it.
struct Entry {
    path: PathBuf,
    name: String,
    len: u64,
    modified: SystemTime,
}

impl Cache {
    /// The cache for a build whose output is `out_path`.
    ///
    /// `KELPC_CACHE_DIR` overrides the directory; otherwise the cache sits
    /// beside the build's other output as `<out dir>/.kelpc-cache`. The
    /// default output path is `build/<stem>`, so the default cache is
    /// `build/.kelpc-cache`. `KELPC_CACHE=off` builds the directory path but
    /// turns lookups and stores off.
    pub fn for_build(out_path: &Path) -> Cache {
        let dir = match std::env::var_os(DIR_ENV) {
            Some(dir) => PathBuf::from(dir),
            None => {
                let parent = match out_path.parent() {
                    Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                    _ => PathBuf::from("."),
                };
                parent.join(CACHE_DIR_NAME)
            }
        };
        Cache::with_dir(dir)
    }

    /// A cache for an explicit directory (the `kelpc cache` subcommand and the
    /// tests' `KELPC_CACHE_DIR`).
    pub fn with_dir(dir: PathBuf) -> Cache {
        let enabled = !matches!(std::env::var(OFF_ENV).as_deref(), Ok("off"));
        Cache {
            dir,
            enabled,
            stats: CacheStats::default(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[cfg(test)]
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// The `<key>.o` path for `key`.
    fn entry_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.o"))
    }

    fn interface_path(&self, module: &str) -> PathBuf {
        let key = sha256_hex(format!("kelpc-front-end-interface-path-1\n{module}").as_bytes());
        self.dir.join(format!("{key}.{INTERFACE_EXT}"))
    }

    /// Persist generated public contracts after a successful front-end pass.
    /// These are compiler-owned artifacts, not source files a project may
    /// check in or import.
    pub fn store_interfaces(
        &self,
        interfaces: &[crate::resolve::ModuleInterface],
    ) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            format!(
                "could not create the cache directory '{}': {e}",
                self.dir.display()
            )
        })?;
        for interface in interfaces {
            let mut text = format!(
                "kelpc-front-end-interface-1\nmodule={}\napi={}\nhash={}\n",
                interface.module, interface.api_hash, interface.hash
            );
            for (module, hash) in &interface.dependencies {
                text.push_str(&format!("dependency={module}:{hash}\n"));
            }
            self.store_bytes(&self.interface_path(&interface.module), text.as_bytes())?;
        }
        Ok(())
    }

    /// Look `key` up: `Some(path)` when a usable entry exists, `None` when the
    /// entry is missing, unreadable, empty or visibly incomplete. A hit and a
    /// miss are counted here, because every lookup is one or the other.
    pub fn lookup(&self, key: &str) -> Option<PathBuf> {
        if !self.enabled {
            return None;
        }
        let path = self.entry_path(key);
        if !is_complete_object(&path) {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.stats.hits.fetch_add(1, Ordering::Relaxed);
        Some(path)
    }

    /// Store `object` as this key's entry, atomically: a temporary file in the
    /// same directory is written and then renamed over `<key>.o`, so a reader
    /// sees either the old entry or the complete new one, never a partial file.
    pub fn store(&self, key: &str, object: &Path) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            format!(
                "could not create the cache directory '{}': {e}",
                self.dir.display()
            )
        })?;
        let bytes = std::fs::read(object).map_err(|e| {
            format!(
                "could not read the unit object '{}' to cache it: {e}",
                object.display()
            )
        })?;
        self.store_bytes(&self.entry_path(key), &bytes)?;
        self.stats.stored.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Install one complete cache artifact with one atomic rename.
    fn store_bytes(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = self.dir.join(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            ENTRY_TMP.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(e) = std::fs::write(&tmp, bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "could not write the cache entry '{}': {e}",
                tmp.display()
            ));
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "could not install the cache entry '{}': {e}",
                path.display()
            ));
        }
        Ok(())
    }

    /// Delete one entry, the caller's recovery from an entry that failed to
    /// link. A missing entry is not an error: the point is only that the next
    /// build does not find it.
    pub fn delete(&self, key: &str) {
        let _ = std::fs::remove_file(self.entry_path(key));
    }

    /// Every object or interface entry, with the metadata eviction sorts by.
    fn entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for dirent in read.flatten() {
            let name = dirent.file_name().to_string_lossy().into_owned();
            if !is_entry_name(&name) {
                continue;
            }
            let path = dirent.path();
            let Ok(meta) = dirent.metadata() else {
                continue;
            };
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(Entry {
                path,
                name,
                len: meta.len(),
                modified,
            });
        }
        out
    }

    /// The number of entries and their total size, for `kelpc cache stats`.
    pub fn entry_count_and_size(&self) -> (usize, u64) {
        let entries = self.entries();
        let total = entries.iter().map(|e| e.len).sum();
        (entries.len(), total)
    }

    /// Delete every entry in the cache directory. A directory that does not
    /// exist is already clear.
    pub fn clear(&self) -> Result<usize, String> {
        if !self.dir.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        let read = std::fs::read_dir(&self.dir).map_err(|e| {
            format!(
                "could not read the cache directory '{}': {e}",
                self.dir.display()
            )
        })?;
        for dirent in read.flatten() {
            let path = dirent.path();
            if path.is_dir() {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(e) => {
                    return Err(format!(
                        "could not remove the cache entry '{}': {e}",
                        path.display()
                    ))
                }
            }
        }
        Ok(removed)
    }

    /// The size cap in bytes (`KELPC_CACHE_MAX_MB`, or its default). The value
    /// is read as a decimal so a test can set a cap smaller than one mebibyte,
    /// which is the only way to reach eviction with a handful of real entries.
    fn max_bytes(&self) -> u64 {
        let mb = std::env::var(MAX_MB_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .unwrap_or(DEFAULT_MAX_MB);
        (mb * 1024.0 * 1024.0) as u64
    }

    /// Bring the cache under its size cap at the end of a build: delete
    /// oldest-modified entries first until the total is under the cap. Ties in
    /// modification time are broken by name so the order is deterministic.
    pub fn enforce_cap(&self) {
        if !self.enabled {
            return;
        }
        self.enforce_cap_to(self.max_bytes());
    }

    /// [`enforce_cap`] against an explicit byte cap, which is the half a test
    /// can drive without touching the process environment.
    ///
    /// [`enforce_cap`]: Cache::enforce_cap
    fn enforce_cap_to(&self, cap: u64) {
        let mut entries = self.entries();
        let mut total: u64 = entries.iter().map(|e| e.len).sum();
        if total <= cap {
            return;
        }
        entries.sort_by(|a, b| {
            a.modified
                .cmp(&b.modified)
                .then_with(|| a.name.cmp(&b.name))
        });
        for entry in entries {
            if total <= cap {
                break;
            }
            if std::fs::remove_file(&entry.path).is_ok() {
                total = total.saturating_sub(entry.len);
            }
        }
    }

    /// Print the one-line build summary when `KELPC_CACHE_STATS=1`.
    pub fn report_stats(&self) {
        if !matches!(std::env::var(STATS_ENV).as_deref(), Ok("1")) {
            return;
        }
        eprintln!(
            "kelpc cache: {} hits, {} misses, {} stored",
            self.stats.hits(),
            self.stats.misses(),
            self.stats.stored()
        );
    }
}

/// Whether `name` is a cache entry: `<64 hex>.o` or `<64 hex>.kpi`.
fn is_entry_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(ext, "o" | INTERFACE_EXT)
        && stem.len() == 64
        && stem.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Whether `path` holds a complete object file.
///
/// A truncated object is the one corruption a lookup can see without running
/// the linker: an ELF's section header table sits at `e_shoff` and covers
/// `e_shnum` entries, so a file shorter than that is incomplete whatever its
/// header says. A file that is not ELF (another platform's object format) is
/// accepted when it is non-empty, because this check can say nothing about a
/// format it does not parse. A missing, unreadable or empty file is not usable.
pub fn is_complete_object(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() == 0 {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 64];
    let mut read = 0;
    while read < head.len() {
        match file.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => return false,
        }
    }
    if read < 4 || &head[..4] != b"\x7fELF" {
        // Not ELF: non-empty is all this check can promise.
        return true;
    }
    if read < 64 {
        return false;
    }
    let elf64 = match head[4] {
        2 => true,
        1 => false,
        _ => return false,
    };
    let u16_at = |at: usize| u16::from_le_bytes([head[at], head[at + 1]]);
    let u64_at = |at: usize| {
        u64::from_le_bytes([
            head[at],
            head[at + 1],
            head[at + 2],
            head[at + 3],
            head[at + 4],
            head[at + 5],
            head[at + 6],
            head[at + 7],
        ])
    };
    let (shoff, shentsize, shnum) = if elf64 {
        (u64_at(0x28), u16_at(0x3a) as u64, u16_at(0x3c) as u64)
    } else {
        (u32::from_le_bytes([head[0x20], head[0x21], head[0x22], head[0x23]]) as u64,
         u16_at(0x2e) as u64,
         u16_at(0x30) as u64)
    };
    if shoff == 0 || shnum == 0 || shentsize == 0 {
        return false;
    }
    meta.len() >= shoff.saturating_add(shentsize.saturating_mul(shnum))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn fields<'a>() -> KeyFields<'a> {
        KeyFields {
            triple: "x86_64-unknown-linux-gnu",
            cpu: "x86-64",
            features: "",
            reloc: "pic",
            code_model: "default",
            opt: "default<O2>",
            debug: "dwarf",
            compiler: "kelpc test llvm 21.1.0",
        }
    }

    #[test]
    fn the_key_is_a_hex_sha256_and_is_stable() {
        let key = object_key(&fields(), "define void @main() { ret void }\n");
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(key, object_key(&fields(), "define void @main() { ret void }\n"));
    }

    #[test]
    fn every_field_and_the_ir_change_the_key() {
        let base = object_key(&fields(), "ir");
        let differ = |change: fn(&mut KeyFields)| {
            let mut f = fields();
            change(&mut f);
            assert_ne!(base, object_key(&f, "ir"), "a changed field shared a key");
        };
        differ(|f| f.triple = "aarch64-unknown-linux-gnu");
        differ(|f| f.cpu = "generic");
        differ(|f| f.features = "+m");
        differ(|f| f.reloc = "static");
        differ(|f| f.code_model = "medium");
        differ(|f| f.opt = "default<O0>");
        differ(|f| f.debug = "none");
        differ(|f| f.compiler = "kelpc other");
        assert_ne!(base, object_key(&fields(), "ir2"));
    }

    #[test]
    fn a_field_value_cannot_impersonate_a_later_field() {
        // The labelled, length-prefixed layout is what stops a value holding a
        // newline from colliding with the fields after it.
        let mut a = fields();
        a.cpu = "x\nopt=default<O0>";
        let mut b = fields();
        b.opt = "default<O0>";
        assert_ne!(object_key(&a, "ir"), object_key(&b, "ir"));
    }

    #[test]
    fn only_hex_object_names_are_entries() {
        assert!(is_entry_name(&format!("{}.o", "a".repeat(64))));
        assert!(is_entry_name(&format!("{}.kpi", "a".repeat(64))));
        assert!(!is_entry_name(&format!("{}.o", "A".repeat(64))));
        assert!(!is_entry_name(&format!("{}.kpi", "A".repeat(64))));
        assert!(!is_entry_name(&format!("{}.o", "a".repeat(63))));
        assert!(!is_entry_name("main.o"));
        assert!(!is_entry_name(&format!(".{}.o.tmp-1-2", "a".repeat(64))));
    }

    /// A small but complete ELF64 image: 64-byte header plus one section
    /// header. Enough for the completeness check, not something a linker
    /// would accept.
    fn elf(len_override: Option<u64>) -> Vec<u8> {
        let shentsize = 64u16;
        let shnum = 1u16;
        let shoff = 64u64;
        let mut bytes = vec![0u8; (shoff + shentsize as u64 * shnum as u64) as usize];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2; // ELF64
        bytes[0x28..0x30].copy_from_slice(&shoff.to_le_bytes());
        bytes[0x3a..0x3c].copy_from_slice(&shentsize.to_le_bytes());
        bytes[0x3c..0x3e].copy_from_slice(&shnum.to_le_bytes());
        if let Some(len) = len_override {
            bytes.truncate(len as usize);
        }
        bytes
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        std::fs::create_dir_all(dir).expect("create dir");
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write file");
        path
    }

    #[test]
    fn a_complete_elf_is_usable_and_a_truncated_one_is_not() {
        let dir = std::env::temp_dir().join(format!("kelpc_cache_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let full = write(&dir, "full.o", &elf(None));
        assert!(is_complete_object(&full));
        let cut = write(&dir, "cut.o", &elf(Some(70)));
        assert!(!is_complete_object(&cut));
        let empty = write(&dir, "empty.o", &[]);
        assert!(!is_complete_object(&empty));
        assert!(!is_complete_object(&dir.join("missing.o")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lookup_of_a_missing_entry_is_a_miss_and_a_full_one_is_a_hit() {
        let dir = std::env::temp_dir().join(format!("kelpc_cache_test2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::with_dir(dir.clone());
        let key = "b".repeat(64);
        assert!(cache.lookup(&key).is_none());
        assert_eq!((cache.stats().hits(), cache.stats().misses()), (0, 1));
        write(&dir, &format!("{key}.o"), &elf(None));
        assert!(cache.lookup(&key).is_some());
        assert_eq!((cache.stats().hits(), cache.stats().misses()), (1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interfaces_are_atomically_written_as_generated_cache_entries() {
        let dir = std::env::temp_dir().join(format!("kelpc_interface_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::with_dir(dir.clone());
        cache.store_interfaces(&[crate::resolve::ModuleInterface {
            module: "cacheproj/alpha".into(),
            api_hash: "api".into(),
            hash: "full".into(),
            dependencies: vec![("std/io".into(), "std-hash".into())],
        }]).expect("store interface");
        let entries = cache.entries();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].name.ends_with(".kpi"));
        let text = std::fs::read_to_string(&entries[0].path).expect("read interface");
        assert_eq!(text, "kelpc-front-end-interface-1\nmodule=cacheproj/alpha\napi=api\nhash=full\ndependency=std/io:std-hash\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cap_evicts_oldest_modified_entries_first() {
        let dir = std::env::temp_dir().join(format!("kelpc_cache_test3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::with_dir(dir.clone());
        let base = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let names: Vec<String> = (0..4).map(|i| format!("{:064x}.o", i)).collect();
        for (i, name) in names.iter().enumerate() {
            let path = write(&dir, name, &elf(None));
            let when = base + std::time::Duration::from_secs(i as u64);
            File::options()
                .write(true)
                .open(&path)
                .expect("reopen")
                .set_modified(when)
                .expect("set mtime");
        }
        let size = std::fs::metadata(dir.join(&names[0])).unwrap().len();
        // A cap that holds three entries: the oldest one has to go.
        cache.enforce_cap_to(size * 3);
        assert!(!dir.join(&names[0]).exists(), "the oldest entry was kept");
        for name in &names[1..] {
            assert!(dir.join(name).exists(), "'{name}' was evicted too");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

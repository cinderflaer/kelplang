//! `std/fs`: file and directory primitives. The panicking bodies and the `try_*`
//! twins share the same POSIX and stdio calls the C bodies made. Every `try_*`
//! body writes 0 through `err` on success and the errno of the failing call
//! otherwise.

use core::ffi::{c_char, c_int, c_void};
use core::ptr::{null, null_mut};

use crate::abi::{Bytes, String as KString};
use crate::alloc::kelp_bytes_alloc_raw;
use crate::panic::{kelp_panic, kelp_panic_at, kelp_panic_errno};
use crate::text::{kelp_bytes_drop, kelp_String__from_bytes, kelp_String__from_str, kelp_utf8_valid};

const PATH_MAX_LEN: usize = libc::PATH_MAX as usize;

unsafe extern "C" {
    fn scandir(
        dirp: *const c_char,
        namelist: *mut *mut *mut libc::dirent,
        filter: Option<unsafe extern "C" fn(*const libc::dirent) -> c_int>,
        compar: Option<unsafe extern "C" fn(*const *const libc::dirent, *const *const libc::dirent) -> c_int>,
    ) -> c_int;
    fn alphasort(a: *const *const libc::dirent, b: *const *const libc::dirent) -> c_int;
}

fn errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

/// `fopen(path, mode)` on a Kelp `&str` path.
unsafe fn fopen(path: *const c_char, mode: &core::ffi::CStr) -> *mut libc::FILE {
    unsafe { libc::fopen(path, mode.as_ptr()) }
}

/// Writes `len` bytes of `data` to an open stream. `true` when all of them went
/// out. A zero-length write is always `true`, as the C bodies had it.
unsafe fn write_all(f: *mut libc::FILE, data: *const u8, len: usize) -> bool {
    len == 0 || unsafe { libc::fwrite(data.cast::<c_void>(), 1, len, f) } == len
}

/// `stat` on `path`, or `None` when the call fails (the errno is left set).
unsafe fn stat_of(path: *const c_char) -> Option<libc::stat> {
    let mut st: libc::stat = unsafe { core::mem::zeroed() };
    if unsafe { libc::stat(path, &mut st) } == 0 {
        Some(st)
    } else {
        None
    }
}

fn is_reg(mode: libc::mode_t) -> bool {
    mode & libc::S_IFMT == libc::S_IFREG
}

fn is_dir(mode: libc::mode_t) -> bool {
    mode & libc::S_IFMT == libc::S_IFDIR
}

fn is_lnk(mode: libc::mode_t) -> bool {
    mode & libc::S_IFMT == libc::S_IFLNK
}

/// The `open(2)` flags of a `fs.open` mode, or `None` for an unknown mode.
unsafe fn mode_flags(mode: *const c_char) -> Option<c_int> {
    let m = unsafe { core::ffi::CStr::from_ptr(mode) }.to_bytes();
    match m {
        b"r" => Some(libc::O_RDONLY),
        b"w" => Some(libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC),
        b"a" => Some(libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND),
        b"rw" => Some(libc::O_RDWR | libc::O_CREAT),
        _ => None,
    }
}

/// The names in `path` other than `.` and `..`, sorted with `alphasort`, joined
/// by `\n` with no trailing newline. `Err` carries the errno of `scandir`.
unsafe fn list_names(path: *const c_char) -> Result<Bytes, c_int> {
    unsafe {
        let mut names: *mut *mut libc::dirent = null_mut();
        let n = scandir(path, &mut names, None, Some(alphasort));
        if n < 0 {
            return Err(errno());
        }
        let is_dots = |d: *mut libc::dirent| {
            let name = (*d).d_name.as_ptr();
            libc::strcmp(name, c".".as_ptr()) == 0 || libc::strcmp(name, c"..".as_ptr()) == 0
        };
        let mut total = 0usize;
        for i in 0..n as usize {
            let d = *names.add(i);
            if !is_dots(d) {
                total += libc::strlen((*d).d_name.as_ptr()) + 1;
            }
        }
        let b = kelp_bytes_alloc_raw(total);
        let mut at = 0usize;
        for i in 0..n as usize {
            let d = *names.add(i);
            if !is_dots(d) {
                let name = (*d).d_name.as_ptr();
                let len = libc::strlen(name);
                core::ptr::copy_nonoverlapping(name.cast::<u8>(), b.data.add(at), len);
                at += len;
                *b.data.add(at) = b'\n';
                at += 1;
            }
            libc::free(d.cast());
        }
        libc::free(names.cast());
        if at > 0 {
            at -= 1;
        }
        *b.data.add(at) = 0;
        Ok(Bytes {
            data: b.data,
            len: at,
            cap: b.cap,
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__read_raw(path: *const c_char) -> Bytes {
    unsafe {
        let f = fopen(path, c"rb");
        if f.is_null() {
            kelp_panic_errno(c"fs.read: cannot open file".as_ptr());
        }
        if libc::fseek(f, 0, libc::SEEK_END) != 0 {
            let e = errno();
            libc::fclose(f);
            kelp_panic_at(c"fs.read: cannot seek".as_ptr(), e);
        }
        let n = libc::ftell(f);
        if n < 0 {
            let e = errno();
            libc::fclose(f);
            kelp_panic_at(c"fs.read: cannot size file".as_ptr(), e);
        }
        libc::rewind(f);
        // Zero-filled and NUL-terminated, so `Bytes.as_str` works on the result.
        let b = kelp_bytes_alloc_raw(n as usize);
        if n > 0 && libc::fread(b.data.cast(), 1, n as usize, f) != n as usize {
            libc::fclose(f);
            kelp_panic(c"fs.read: short read".as_ptr());
        }
        libc::fclose(f);
        b
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__write_raw(path: *const c_char, data: *const Bytes) {
    unsafe {
        let f = fopen(path, c"wb");
        if f.is_null() {
            kelp_panic_errno(c"fs.write: cannot open file".as_ptr());
        }
        if !write_all(f, (*data).data, (*data).len) {
            libc::fclose(f);
            kelp_panic(c"fs.write: short write".as_ptr());
        }
        libc::fclose(f);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__write_text_raw(path: *const c_char, text: *const KString) {
    unsafe {
        let f = fopen(path, c"wb");
        if f.is_null() {
            kelp_panic_errno(c"fs.write_text: cannot open file".as_ptr());
        }
        if !write_all(f, (*text).data, (*text).len) {
            libc::fclose(f);
            kelp_panic(c"fs.write_text: short write".as_ptr());
        }
        libc::fclose(f);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__exists_raw(path: *const c_char) -> bool {
    unsafe { stat_of(path).is_some() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__is_file_raw(path: *const c_char) -> bool {
    unsafe { stat_of(path).is_some_and(|st| is_reg(st.st_mode)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__is_dir_raw(path: *const c_char) -> bool {
    unsafe { stat_of(path).is_some_and(|st| is_dir(st.st_mode)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__size_raw(path: *const c_char) -> u64 {
    unsafe {
        match stat_of(path) {
            Some(st) => st.st_size as u64,
            None => kelp_panic_errno(c"fs.size: cannot stat file".as_ptr()),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__remove_raw(path: *const c_char) {
    unsafe {
        if libc::remove(path) != 0 {
            kelp_panic_errno(c"fs.remove: cannot remove file".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__rename_raw(from: *const c_char, to: *const c_char) {
    unsafe {
        if libc::rename(from, to) != 0 {
            kelp_panic_errno(c"fs.rename: cannot rename file".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__mkdir_raw(path: *const c_char) {
    unsafe {
        if libc::mkdir(path, 0o777) != 0 {
            kelp_panic_errno(c"fs.mkdir: cannot create directory".as_ptr());
        }
    }
}

/// Creates every prefix of `path` in order. An existing component is success;
/// any other failure is a panic. Done here so the grant is checked once on the
/// whole path, never on a prefix above it.
unsafe fn mkdir_all(path: *const c_char) -> Result<(), c_int> {
    unsafe {
        let mut buf = [0 as c_char; PATH_MAX_LEN];
        let n = libc::strlen(path);
        if n == 0 || n >= buf.len() {
            return Err(if n == 0 { libc::EINVAL } else { libc::ENAMETOOLONG });
        }
        core::ptr::copy_nonoverlapping(path, buf.as_mut_ptr(), n + 1);
        for i in 1..=n {
            if buf[i] == b'/' as c_char || buf[i] == 0 {
                let saved = buf[i];
                buf[i] = 0;
                if libc::mkdir(buf.as_ptr(), 0o777) != 0 && errno() != libc::EEXIST {
                    return Err(errno());
                }
                buf[i] = saved;
            }
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__mkdir_all_raw(path: *const c_char) {
    unsafe {
        let n = libc::strlen(path);
        if n == 0 || n >= PATH_MAX_LEN {
            kelp_panic(c"fs.mkdir_all: path is empty or too long".as_ptr());
        }
        if mkdir_all(path).is_err() {
            kelp_panic_errno(c"fs.mkdir_all: cannot create directory".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__append_raw(path: *const c_char, data: *const Bytes) {
    unsafe {
        let f = fopen(path, c"ab");
        if f.is_null() {
            kelp_panic_errno(c"fs.append: cannot open file".as_ptr());
        }
        if !write_all(f, (*data).data, (*data).len) {
            libc::fclose(f);
            kelp_panic(c"fs.append: short write".as_ptr());
        }
        libc::fclose(f);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__list_raw(path: *const c_char) -> KString {
    unsafe {
        match list_names(path) {
            Ok(b) => kelp_String__from_bytes(b),
            Err(_) => kelp_panic_errno(c"fs.list: cannot read directory".as_ptr()),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__sync_raw(path: *const c_char) {
    unsafe {
        let fd = libc::open(path, libc::O_RDONLY);
        if fd < 0 {
            kelp_panic_errno(c"fs.sync: cannot open file".as_ptr());
        }
        if libc::fsync(fd) != 0 {
            let e = errno();
            libc::close(fd);
            kelp_panic_at(c"fs.sync: fsync failed".as_ptr(), e);
        }
        libc::close(fd);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__open_raw(path: *const c_char, mode: *const c_char) -> c_int {
    unsafe {
        let Some(flags) = mode_flags(mode) else {
            kelp_panic(c"fs.open: mode must be \"r\", \"w\", \"a\" or \"rw\"".as_ptr());
        };
        let fd = libc::open(path, flags, 0o666 as libc::c_uint);
        if fd < 0 {
            kelp_panic_errno(c"fs.open: cannot open file".as_ptr());
        }
        fd
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__symlink_raw(target: *const c_char, link: *const c_char) {
    unsafe {
        if libc::symlink(target, link) != 0 {
            kelp_panic_errno(c"fs.symlink: cannot create symlink".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__copy_raw(from: *const c_char, to: *const c_char) {
    unsafe {
        let inp = libc::open(from, libc::O_RDONLY);
        if inp < 0 {
            kelp_panic_errno(c"fs.copy: cannot open source".as_ptr());
        }
        let Some(st) = stat_fd(inp) else {
            let e = errno();
            libc::close(inp);
            kelp_panic_at(c"fs.copy: cannot stat source".as_ptr(), e);
        };
        let out = libc::open(to, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o666 as libc::c_uint);
        if out < 0 {
            let e = errno();
            libc::close(inp);
            kelp_panic_at(c"fs.copy: cannot open destination".as_ptr(), e);
        }
        let mut at: libc::off_t = 0;
        while at < st.st_size {
            let n = libc::sendfile(out, inp, &mut at, (st.st_size - at) as usize);
            if n < 0 {
                let e = errno();
                libc::close(inp);
                libc::close(out);
                kelp_panic_at(c"fs.copy: copy failed".as_ptr(), e);
            }
            if n == 0 {
                break;
            }
        }
        libc::close(inp);
        libc::close(out);
    }
}

/// `fstat` on a descriptor, or `None` on failure.
unsafe fn stat_fd(fd: c_int) -> Option<libc::stat> {
    let mut st: libc::stat = unsafe { core::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } == 0 {
        Some(st)
    } else {
        None
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__touch_raw(path: *const c_char) {
    unsafe {
        let fd = libc::open(path, libc::O_WRONLY | libc::O_CREAT, 0o666 as libc::c_uint);
        if fd < 0 {
            kelp_panic_errno(c"fs.touch: cannot create file".as_ptr());
        }
        libc::close(fd);
        if libc::utimensat(libc::AT_FDCWD, path, null(), 0) != 0 {
            kelp_panic_errno(c"fs.touch: cannot update timestamp".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__set_mode_raw(path: *const c_char, mode: u32) {
    unsafe {
        if libc::chmod(path, (mode & 0o7777) as libc::mode_t) != 0 {
            kelp_panic_errno(c"fs.set_mode: cannot change mode".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__temp_path_raw(prefix: *const c_char) -> KString {
    unsafe {
        let mut tmpl = [0 as c_char; PATH_MAX_LEN];
        // `%sXXXXXX`: the prefix, then six placeholders. The write fails when the
        // whole template would not fit with its terminator.
        let plen = libc::strlen(prefix);
        if plen + 6 >= tmpl.len() {
            kelp_panic(c"fs.temp_path: prefix is too long".as_ptr());
        }
        core::ptr::copy_nonoverlapping(prefix, tmpl.as_mut_ptr(), plen);
        core::ptr::copy_nonoverlapping(c"XXXXXX".as_ptr(), tmpl.as_mut_ptr().add(plen), 7);
        let fd = libc::mkstemp(tmpl.as_mut_ptr());
        if fd < 0 {
            kelp_panic_errno(c"fs.temp_path: cannot create temporary file".as_ptr());
        }
        libc::close(fd);
        kelp_String__from_str(tmpl.as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__mtime_raw(path: *const c_char) -> i64 {
    unsafe {
        match stat_of(path) {
            Some(st) => st.st_mtime as i64,
            None => kelp_panic_errno(c"fs.metadata: cannot stat file".as_ptr()),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__mode_raw(path: *const c_char) -> u32 {
    unsafe {
        match stat_of(path) {
            Some(st) => (st.st_mode & 0o7777) as u32,
            None => kelp_panic_errno(c"fs.metadata: cannot stat file".as_ptr()),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__metadata_raw(
    path: *const c_char,
    size: *mut u64,
    mtime: *mut i64,
    mode: *mut u32,
    is_dir_out: *mut bool,
    is_file_out: *mut bool,
) {
    unsafe {
        let Some(st) = stat_of(path) else {
            kelp_panic_errno(c"fs.metadata: cannot stat file".as_ptr());
        };
        *size = st.st_size as u64;
        *mtime = st.st_mtime as i64;
        *mode = (st.st_mode & 0o7777) as u32;
        *is_dir_out = is_dir(st.st_mode);
        *is_file_out = is_reg(st.st_mode);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__is_symlink_raw(path: *const c_char) -> bool {
    unsafe {
        let mut st: libc::stat = core::mem::zeroed();
        libc::lstat(path, &mut st) == 0 && is_lnk(st.st_mode)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__read_link_raw(path: *const c_char) -> KString {
    unsafe {
        let mut buf = [0 as c_char; PATH_MAX_LEN];
        let n = libc::readlink(path, buf.as_mut_ptr(), buf.len());
        if n < 0 {
            kelp_panic_errno(c"fs.read_link: cannot read link".as_ptr());
        }
        if n as usize >= buf.len() {
            kelp_panic(c"fs.read_link: target is too long".as_ptr());
        }
        buf[n as usize] = 0;
        kelp_String__from_str(buf.as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__cwd_raw() -> KString {
    unsafe {
        let mut buf = [0 as c_char; PATH_MAX_LEN];
        if libc::getcwd(buf.as_mut_ptr(), buf.len()).is_null() {
            kelp_panic_errno(c"fs.cwd: cannot read the current directory".as_ptr());
        }
        kelp_String__from_str(buf.as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_read_raw(path: *const c_char, err: *mut c_int) -> Bytes {
    unsafe {
        *err = 0;
        let f = fopen(path, c"rb");
        if f.is_null() {
            *err = errno();
            return kelp_bytes_alloc_raw(0);
        }
        if libc::fseek(f, 0, libc::SEEK_END) != 0 {
            *err = errno();
            libc::fclose(f);
            return kelp_bytes_alloc_raw(0);
        }
        let n = libc::ftell(f);
        if n < 0 {
            *err = errno();
            libc::fclose(f);
            return kelp_bytes_alloc_raw(0);
        }
        libc::rewind(f);
        let mut b = kelp_bytes_alloc_raw(n as usize);
        if n > 0 && libc::fread(b.data.cast(), 1, n as usize, f) != n as usize {
            let e = errno();
            libc::fclose(f);
            kelp_bytes_drop(&mut b);
            *err = if e != 0 { e } else { libc::EIO };
            return kelp_bytes_alloc_raw(0);
        }
        libc::fclose(f);
        b
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_open_raw(path: *const c_char, mode: *const c_char, err: *mut c_int) -> c_int {
    unsafe {
        let Some(flags) = mode_flags(mode) else {
            *err = libc::EINVAL;
            return -1;
        };
        let fd = libc::open(path, flags, 0o666 as libc::c_uint);
        if fd < 0 {
            *err = errno();
            return -1;
        }
        *err = 0;
        fd
    }
}

/// Writes `data` to `path` opened with `mode` (`wb` or `ab`), setting `err` as
/// the `try_write`/`try_append` bodies did.
unsafe fn try_write_bytes(path: *const c_char, mode: &core::ffi::CStr, data: *const u8, len: usize, err: *mut c_int) {
    unsafe {
        *err = 0;
        let f = fopen(path, mode);
        if f.is_null() {
            *err = errno();
            return;
        }
        if !write_all(f, data, len) {
            let e = if errno() != 0 { errno() } else { libc::EIO };
            libc::fclose(f);
            *err = e;
            return;
        }
        if libc::fclose(f) != 0 {
            *err = errno();
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_write_raw(path: *const c_char, data: *const Bytes, err: *mut c_int) {
    unsafe { try_write_bytes(path, c"wb", (*data).data, (*data).len, err) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_write_text_raw(path: *const c_char, text: *const KString, err: *mut c_int) {
    unsafe { try_write_bytes(path, c"wb", (*text).data, (*text).len, err) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_append_raw(path: *const c_char, data: *const Bytes, err: *mut c_int) {
    unsafe { try_write_bytes(path, c"ab", (*data).data, (*data).len, err) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_remove_raw(path: *const c_char, err: *mut c_int) {
    unsafe { *err = if libc::remove(path) != 0 { errno() } else { 0 } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_rename_raw(from: *const c_char, to: *const c_char, err: *mut c_int) {
    unsafe { *err = if libc::rename(from, to) != 0 { errno() } else { 0 } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_symlink_raw(target: *const c_char, link: *const c_char, err: *mut c_int) {
    unsafe { *err = if libc::symlink(target, link) != 0 { errno() } else { 0 } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_mkdir_all_raw(path: *const c_char, err: *mut c_int) {
    unsafe {
        *err = 0;
        if let Err(e) = mkdir_all(path) {
            *err = e;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__try_list_raw(path: *const c_char, err: *mut c_int) -> KString {
    unsafe {
        *err = 0;
        match list_names(path) {
            Ok(b) => kelp_String__from_bytes(b),
            Err(e) => {
                *err = e;
                kelp_String__from_str(c"".as_ptr())
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__fs__utf8_valid_raw(data: *const Bytes) -> bool {
    unsafe { kelp_utf8_valid((*data).data, (*data).len) }
}

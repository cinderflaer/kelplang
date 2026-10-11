//! `std/process`: argv, file tests, child processes, and exit.
//!
//! A `spawn` body runs `fork` and then, in the child, `chdir`, a `malloc`ed
//! `argv`, and `execvp`. The child leaves with `_exit`, so an inherited stdio
//! buffer is never flushed a second time.

use core::ffi::{c_char, c_int};
use core::ptr::null_mut;

use crate::abi::String as KString;
use crate::panic::{kelp_panic, kelp_panic_errno};
use crate::proc::{kelp_argc, kelp_argv, kelp_child_add, kelp_child_has, kelp_child_remove};
use crate::text::kelp_String__from_str;

unsafe extern "C" {
    static mut stderr: *mut libc::FILE;
    fn execvp(file: *const c_char, argv: *const *mut c_char) -> c_int;
    fn chdir(path: *const c_char) -> c_int;
    fn access(path: *const c_char, mode: c_int) -> c_int;
    fn fork() -> libc::pid_t;
    fn pipe(fds: *mut c_int) -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn _exit(code: c_int) -> !;
}

/// A `List<String>` as the generated code lays it out: a pointer to the
/// elements, then the length and capacity. Each element is a `String` whose
/// `data` is NUL-terminated.
#[repr(C)]
pub struct ListString {
    pub data: *const KString,
    pub len: usize,
    pub cap: usize,
}

fn errno() -> c_int {
    unsafe { *libc::__errno_location() }
}

/// The child's side of a spawn: enter `dir` when it is set, build the argument
/// vector, and exec. Never returns; every failure leaves with `_exit`.
///
/// `msg_prefix` is the name the messages use (`process.spawn` or
/// `process.spawn_inherit`), so the two bodies keep their own text.
unsafe fn exec_child(dir: *const c_char, program: *const c_char, args: *const ListString, msg_prefix: &str) -> ! {
    unsafe {
        let enter_msg = if msg_prefix == "process.spawn" {
            c"process.spawn_in: cannot enter '%s'\n"
        } else {
            c"process.spawn_inherit_in: cannot enter '%s'\n"
        };
        let oom_msg = if msg_prefix == "process.spawn" {
            c"process.spawn: out of memory\n"
        } else {
            c"process.spawn_inherit: out of memory\n"
        };
        let exec_msg = if msg_prefix == "process.spawn" {
            c"process.spawn: cannot exec '%s'\n"
        } else {
            c"process.spawn_inherit: cannot exec '%s'\n"
        };
        // The per-call working directory (MK5-S2-M2): in the child, between
        // fork and exec, so the parent's directory is unchanged. An empty `dir`
        // keeps the parent's. 126 is reserved for this failure.
        if *dir != 0 && chdir(dir) != 0 {
            libc::fprintf(stderr, enter_msg.as_ptr(), dir);
            _exit(126);
        }
        let n = (*args).len;
        let argv = libc::malloc((n + 2) * core::mem::size_of::<*mut c_char>()) as *mut *mut c_char;
        if argv.is_null() {
            libc::fprintf(stderr, oom_msg.as_ptr());
            _exit(127);
        }
        *argv = program as *mut c_char;
        for i in 0..n {
            *argv.add(i + 1) = (*(*args).data.add(i)).data as *mut c_char;
        }
        *argv.add(n + 1) = null_mut();
        execvp(program, argv);
        // `exec` did not replace this image, so the array is still ours and this
        // child is about to leave: free it before `_exit`.
        libc::free(argv.cast());
        libc::fprintf(stderr, exec_msg.as_ptr(), program);
        _exit(127);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__arg_count_raw() -> usize {
    unsafe { kelp_argc as usize }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__arg_at_raw(i: usize) -> KString {
    unsafe {
        if i >= kelp_argc as usize {
            kelp_panic(c"process.arg_at: index out of bounds".as_ptr());
        }
        kelp_String__from_str(*kelp_argv.add(i))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__exists_raw(path: *const c_char) -> bool {
    unsafe {
        let mut st: libc::stat = core::mem::zeroed();
        libc::stat(path, &mut st) == 0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__executable_raw(path: *const c_char) -> bool {
    unsafe {
        let mut st: libc::stat = core::mem::zeroed();
        if libc::stat(path, &mut st) != 0 {
            return false;
        }
        // A directory carries an execute bit too, so the regular-file test is
        // what keeps `process.executable("/tmp")` false.
        st.st_mode & libc::S_IFMT == libc::S_IFREG && access(path, libc::X_OK) == 0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__is_dir_raw(path: *const c_char) -> bool {
    unsafe {
        let mut st: libc::stat = core::mem::zeroed();
        libc::stat(path, &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFDIR
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__spawn_raw(
    dir: *const c_char,
    program: *const c_char,
    args: *const ListString,
    pid: *mut i32,
    stdin_fd: *mut i32,
    stdout_fd: *mut i32,
    stderr_fd: *mut i32,
) {
    unsafe {
        let mut in_pipe = [0 as c_int; 2];
        let mut out_pipe = [0 as c_int; 2];
        let mut err_pipe = [0 as c_int; 2];
        if pipe(in_pipe.as_mut_ptr()) != 0 || pipe(out_pipe.as_mut_ptr()) != 0 || pipe(err_pipe.as_mut_ptr()) != 0 {
            kelp_panic_errno(c"process.spawn: cannot create a pipe".as_ptr());
        }
        let child = fork();
        if child < 0 {
            kelp_panic_errno(c"process.spawn: fork failed".as_ptr());
        }
        if child == 0 {
            // The child: wire the pipes to stdio, then close every copy it does
            // not need.
            dup2(in_pipe[0], 0);
            dup2(out_pipe[1], 1);
            dup2(err_pipe[1], 2);
            for fd in [in_pipe[0], in_pipe[1], out_pipe[0], out_pipe[1], err_pipe[0], err_pipe[1]] {
                libc::close(fd);
            }
            exec_child(dir, program, args, "process.spawn");
        }
        // The parent keeps only its own ends: the child's stdin write end, and
        // the read ends of its output.
        libc::close(in_pipe[0]);
        libc::close(out_pipe[1]);
        libc::close(err_pipe[1]);
        kelp_child_add(child);
        *pid = child;
        *stdin_fd = in_pipe[1];
        *stdout_fd = out_pipe[0];
        *stderr_fd = err_pipe[0];
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__spawn_inherit_raw(
    dir: *const c_char,
    program: *const c_char,
    args: *const ListString,
    pid: *mut i32,
) {
    unsafe {
        // Buffered stdio is flushed first so the parent's earlier output is not
        // reordered behind the child's.
        libc::fflush(null_mut());
        let child = fork();
        if child < 0 {
            kelp_panic_errno(c"process.spawn_inherit: fork failed".as_ptr());
        }
        if child == 0 {
            exec_child(dir, program, args, "process.spawn_inherit");
        }
        kelp_child_add(child);
        *pid = child;
    }
}

const WNOHANG: c_int = 1;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__reap_raw(pid: i32, block: bool, signal: *mut i32) -> i32 {
    unsafe {
        // The -1 is "still running", not a user-visible sentinel: `Child.is_running`
        // is the only caller that asks for it.
        if !kelp_child_has(pid) {
            kelp_panic(c"process.wait: not a child this program started".as_ptr());
        }
        let mut status: c_int = 0;
        let mut r;
        loop {
            r = libc::waitpid(pid, &mut status, if block { 0 } else { WNOHANG });
            if !(r < 0 && errno() == libc::EINTR) {
                break;
            }
        }
        if r == 0 {
            return -1;
        }
        if r < 0 {
            kelp_panic_errno(c"process.wait: waitpid failed".as_ptr());
        }
        kelp_child_remove(pid);
        *signal = 0;
        let term = status & 0x7f;
        if term == 0 {
            // WIFEXITED: the low seven bits are zero.
            return (status >> 8) & 0xff;
        }
        if term != 0x7f {
            // WIFSIGNALED: killed by a signal other than a stop.
            *signal = term;
            return 128 + term;
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__kill_raw(pid: i32, signal: i32) {
    unsafe {
        if !kelp_child_has(pid) {
            kelp_panic(c"process.kill: not a child this program started".as_ptr());
        }
        if libc::kill(pid, signal) != 0 {
            kelp_panic_errno(c"process.kill: kill failed".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__exit_raw(code: i32) {
    unsafe { libc::exit(code) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__process__getpid_raw() -> i64 {
    unsafe { libc::getpid() as i64 }
}

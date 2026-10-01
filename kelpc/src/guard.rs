//! The compiler's own memory floor: a watchdog that ends a build with a clear
//! message when the machine is nearly out of memory, instead of leaving the
//! kernel's out-of-memory killer to pick a victim (on a small host that victim
//! is often not the compiler, and can be a whole session).
//!
//! `kelpc build` and `kelpc run` start it. Every 200 ms it reads `MemAvailable`
//! from `/proc/meminfo`; below the floor it prints one line, kills the `cc` /
//! `ld.lld` children the driver spawned (they are the biggest consumers and
//! would otherwise outlive `kelpc`), and exits with status 75. `kelpc run` stops
//! it before it starts the compiled program, so the program's own memory use is
//! never policed.
//!
//! The floor is `KELPC_MIN_AVAILABLE_MB` (default 384, `0` turns the guard
//! off). Only Linux is covered: elsewhere there is no `/proc/meminfo` and the
//! guard does nothing.

use std::io;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// The exit status of a build the guard ended (`EX_TEMPFAIL`: the same input
/// can succeed once memory is free).
pub const EXIT_LOW_MEMORY: i32 = 75;

const DEFAULT_FLOOR_MB: u64 = 384;
const POLL: Duration = Duration::from_millis(200);

static STOPPED: AtomicBool = AtomicBool::new(false);
static CHILDREN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

unsafe extern "C" {
    safe fn kill(pid: i32, sig: i32) -> i32;
    safe fn _exit(status: i32) -> !;
}
const SIGKILL: i32 = 9;

/// `MemAvailable` in kibibytes from the text of `/proc/meminfo`, or `None` if
/// the line is absent or malformed.
pub fn available_kb(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .and_then(|rest| rest.trim().strip_suffix("kB"))
        .and_then(|n| n.trim().parse().ok())
}

/// The floor in MiB from the value of `KELPC_MIN_AVAILABLE_MB`: unset is the
/// default, `0` is off, and an unparsable value is the default too.
pub fn floor_mb(var: Option<&str>) -> u64 {
    match var.map(str::trim) {
        None | Some("") => DEFAULT_FLOOR_MB,
        Some(v) => v.parse().unwrap_or(DEFAULT_FLOOR_MB),
    }
}

/// Starts the watchdog thread (a no-op when the floor is 0 or the host has no
/// `/proc/meminfo`).
pub fn start() {
    let floor = floor_mb(std::env::var("KELPC_MIN_AVAILABLE_MB").ok().as_deref());
    if floor == 0 || std::fs::read_to_string("/proc/meminfo").is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("kelpc-memory-guard".to_string())
        .spawn(move || watch(floor));
    // A thread that cannot start leaves the build unguarded, not failed.
    let _ = spawned;
}

/// Stops the watchdog for good: called before a compiled program runs.
pub fn stop() {
    STOPPED.store(true, Ordering::SeqCst);
}

fn watch(floor_mb: u64) {
    while !STOPPED.load(Ordering::SeqCst) {
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            if let Some(kb) = available_kb(&text) {
                if kb / 1024 < floor_mb && !STOPPED.load(Ordering::SeqCst) {
                    abort(kb / 1024, floor_mb);
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

fn abort(available_mb: u64, floor_mb: u64) -> ! {
    if let Ok(children) = CHILDREN.lock() {
        for pid in children.iter() {
            kill(*pid as i32, SIGKILL);
        }
    }
    eprintln!(
        "error: kelpc stopped itself: only {available_mb} MiB of memory is available (floor {floor_mb} MiB). \
         Free some memory and run it again, or lower the floor with KELPC_MIN_AVAILABLE_MB (0 turns the guard off)."
    );
    // `_exit`, not `std::process::exit`: the main thread is still inside LLVM or a
    // child wait, and running the exit handlers under it corrupts the heap.
    _exit(EXIT_LOW_MEMORY)
}

/// `Command::output` for a tool the driver spawns: the child's pid is
/// registered so the guard can kill it if it has to end the build.
pub trait GuardedOutput {
    fn guarded_output(&mut self) -> io::Result<Output>;
}

impl GuardedOutput for Command {
    fn guarded_output(&mut self) -> io::Result<Output> {
        let child = self
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = child.id();
        if let Ok(mut children) = CHILDREN.lock() {
            children.push(pid);
        }
        let out = child.wait_with_output();
        if let Ok(mut children) = CHILDREN.lock() {
            children.retain(|p| *p != pid);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_is_read_in_kib() {
        let text = "MemTotal:        8019592 kB\nMemFree:          100000 kB\nMemAvailable:    4588012 kB\n";
        assert_eq!(available_kb(text), Some(4588012));
    }

    #[test]
    fn a_missing_or_garbled_line_is_none() {
        assert_eq!(available_kb("MemTotal: 1 kB\n"), None);
        assert_eq!(available_kb("MemAvailable: lots kB\n"), None);
        assert_eq!(available_kb("MemAvailable: 12\n"), None);
    }

    #[test]
    fn the_floor_defaults_and_zero_turns_it_off() {
        assert_eq!(floor_mb(None), 384);
        assert_eq!(floor_mb(Some("")), 384);
        assert_eq!(floor_mb(Some("0")), 0);
        assert_eq!(floor_mb(Some("1024")), 1024);
        assert_eq!(floor_mb(Some("many")), 384);
    }
}

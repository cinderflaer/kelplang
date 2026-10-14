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
//!
//! The REPL runs a resumable watchdog (`start_resumable`). It is armed around
//! each line's build. A low reading while it is armed kills the build's child
//! tools and sets a flag (`take_tripped`) instead of ending the process, so the
//! REPL can refuse the line and keep its session. The compiler's own work in
//! the process (LLVM) cannot be stopped that way, so it is not covered.

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
static RESUMABLE: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
static TRIPPED: AtomicBool = AtomicBool::new(false);

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

/// Starts the watchdog in resumable mode (the REPL's). It never ends the process:
/// a low reading while it is armed trips it (see `take_tripped`). It runs in
/// the same way as `start` otherwise, including the floor and the no-op cases.
pub fn start_resumable() {
    RESUMABLE.store(true, Ordering::SeqCst);
    start();
}

/// Arms the resumable watchdog for one build. Only an armed watchdog trips.
pub fn arm() {
    ARMED.store(true, Ordering::SeqCst);
}

/// Disarms the resumable watchdog, so a compiled program is not policed.
pub fn disarm() {
    ARMED.store(false, Ordering::SeqCst);
}

/// Whether the resumable watchdog tripped since the last call, and clears the
/// flag. A trip kills the build's child tools, so the build fails.
pub fn take_tripped() -> bool {
    TRIPPED.swap(false, Ordering::SeqCst)
}

/// What the watchdog does on one poll. `low` is whether available memory is
/// below the floor; `stopped` is the one-way stop of `stop`.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Abort,
    Trip,
}

/// The watchdog's decision for one poll. A `start` watchdog aborts the process
/// unless it was stopped. A resumable one trips only while armed.
pub fn decide(low: bool, resumable: bool, armed: bool, stopped: bool) -> Action {
    if !low {
        Action::Nothing
    } else if resumable {
        if armed {
            Action::Trip
        } else {
            Action::Nothing
        }
    } else if stopped {
        Action::Nothing
    } else {
        Action::Abort
    }
}

fn watch(floor_mb: u64) {
    while !STOPPED.load(Ordering::SeqCst) {
        let resumable = RESUMABLE.load(Ordering::SeqCst);
        let armed = ARMED.load(Ordering::SeqCst);
        // A resumable watchdog that is not armed has nothing to police, so it
        // does not read the meminfo file at all.
        if !resumable || armed {
            if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
                if let Some(kb) = available_kb(&text) {
                    let available_mb = kb / 1024;
                    let low = available_mb < floor_mb;
                    match decide(low, resumable, armed, STOPPED.load(Ordering::SeqCst)) {
                        Action::Abort => abort(available_mb, floor_mb),
                        Action::Trip => trip(),
                        Action::Nothing => {}
                    }
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Kills every child tool the driver has registered.
fn kill_children() {
    if let Ok(children) = CHILDREN.lock() {
        for pid in children.iter() {
            kill(*pid as i32, SIGKILL);
        }
    }
}

fn trip() {
    TRIPPED.store(true, Ordering::SeqCst);
    kill_children();
}

fn abort(available_mb: u64, floor_mb: u64) -> ! {
    kill_children();
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
    fn a_low_reading_aborts_a_plain_watchdog_unless_it_is_stopped() {
        assert_eq!(decide(true, false, false, false), Action::Abort);
        assert_eq!(decide(true, false, true, false), Action::Abort);
        assert_eq!(decide(true, false, false, true), Action::Nothing);
        assert_eq!(decide(false, false, false, false), Action::Nothing);
    }

    #[test]
    fn a_resumable_watchdog_trips_only_while_armed() {
        assert_eq!(decide(true, true, true, false), Action::Trip);
        assert_eq!(decide(true, true, false, false), Action::Nothing);
        assert_eq!(decide(false, true, true, false), Action::Nothing);
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

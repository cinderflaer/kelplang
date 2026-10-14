//! `kelpc repl`, stage 1a of plans/kelp-repl.md.
//!
//! The command reads lines from stdin, or from `--script FILE`. Each line is one
//! expression or one single-line statement. A line is checked by the same front
//! end `kelpc check` runs, then built as a program of its own (a `main` that
//! holds the line and nothing else) and run. There is no session yet: a line
//! does not see the lines before it, and nothing is replayed (decision D1 is
//! stage 1b). An expression of type Int, Float, Bool or String is printed with
//! `print`; a unit-typed line prints only what it prints itself.
//!
//! The exit status is zero when every line succeeded, and nonzero when one did
//! not (decision D2). A line that fails leaves the session unchanged, which in
//! this stage means nothing is kept.
//!
//! Memory: the REPL runs the compiler's watchdog in resumable mode
//! (`guard::start_resumable`). The plain watchdog ends the whole process when
//! memory runs low, which would lose the session and the temporary files. The
//! resumable one is armed around each line's build: when memory is low it kills
//! the build's child tools (cc, ld.lld), the build fails, and the line is an
//! ordinary line error. Each line also checks `MemAvailable` against the same
//! floor (`KELPC_MIN_AVAILABLE_MB`) before it is built. The compiler's own work
//! in the process (LLVM) is not stopped by the watchdog.
//!
//! Temporary files: a REPL killed by a signal (Ctrl-C has no handler yet, D6)
//! leaves its directory behind. Each start removes the directories of REPL
//! processes that are no longer running (see `sweep_stale`).

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::diag::{Diagnostic, Severity, SourceMap};
use crate::lexer::Span;
use crate::opt::OptLevel;
use crate::types::Ty;
use crate::{build, front_end, guard, infer, resolve};

/// The name every REPL diagnostic is shown under.
const NAME: &str = "<repl>";

/// The unknown-name help of the front end, which tells the user to declare the
/// name first. The REPL has no declarations until stage 3, so it is replaced by
/// `NAME_HELP` when a diagnostic is shown here.
const NAME_HELP_FROM: &str =
    "declare it with val or var before this line, or check the spelling.";

/// The unknown-name help shown in the REPL.
const NAME_HELP: &str =
    "check the spelling; this REPL has no declarations yet (stage 3 adds let).";

const PROMPT: &str = ">>> ";

/// The prefix of each REPL process's temporary directory, followed by its pid.
const DIR_PREFIX: &str = "kelpc-repl-";

/// The text before the typed line in a candidate program. The typed line is
/// always on line 2 and starts at column 1, so a diagnostic's byte offset minus
/// `HEAD.len()` is its offset in the typed line.
const HEAD: &str = "fun main() {\n";

/// The text after the typed line in a candidate program.
const TAIL: &str = "\n}\n";

/// Runs `kelpc repl`. `args` are the arguments after `repl`.
pub fn repl_command(args: &[String]) -> ExitCode {
    let mut script: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--script" {
            let Some(value) = args.get(i + 1) else {
                eprintln!("error: '--script' requires a value");
                return ExitCode::FAILURE;
            };
            script = Some(value.clone());
            i += 2;
        } else {
            eprintln!(
                "error: unexpected argument '{}' for 'repl'; expected --script <file>",
                args[i]
            );
            crate::usage();
            return ExitCode::FAILURE;
        }
    }

    let stdin = io::stdin();
    let (mut input, interactive): (Box<dyn BufRead>, bool) = match &script {
        Some(path) => match File::open(path) {
            Ok(file) => (Box::new(BufReader::new(file)) as Box<dyn BufRead>, false),
            Err(e) => {
                eprintln!("error: cannot read '{path}': {e}");
                return ExitCode::FAILURE;
            }
        },
        None => (Box::new(stdin.lock()) as Box<dyn BufRead>, stdin.is_terminal()),
    };

    let work = match WorkDir::create() {
        Ok(work) => work,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    sweep_stale(&std::env::temp_dir());
    guard::start_resumable();
    let mut failed = false;
    let mut count = 0usize;
    let mut buf = String::new();
    loop {
        if interactive {
            print!("{PROMPT}");
            let _ = io::stdout().flush();
        }
        buf.clear();
        match input.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("error: cannot read input: {e}");
                return ExitCode::FAILURE;
            }
        }
        let line = buf.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_start().strip_prefix(':').map(str::trim) == Some("quit") {
            break;
        }
        count += 1;
        if let Some(message) = refusal(line) {
            eprintln!("{message}");
            failed = true;
            continue;
        }
        if !evaluate(line, count, &work.path) {
            failed = true;
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The message for an input this stage does not take, or `None` for a line that
/// is an expression or a statement. Each message names the stage that adds it.
fn refusal(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if let Some(command) = trimmed.strip_prefix(':') {
        let name = command.split_whitespace().next().unwrap_or("");
        let stage = match name {
            "history" | "save" => "6",
            _ => "3",
        };
        return Some(format!(
            "error: ':{name}' is not available yet; stage {stage} of plans/kelp-repl.md adds it"
        ));
    }
    let word = trimmed
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or("");
    let stage = match word {
        "fun" | "datac" | "enum" | "const" | "static" | "let" => "3",
        "impl" | "import" => "5",
        _ => return None,
    };
    Some(format!(
        "error: '{word}' is not available yet; stage {stage} of plans/kelp-repl.md adds it"
    ))
}

/// Checks, builds and runs one line. Prints the line's diagnostics or its
/// failure, and returns whether the line succeeded.
fn evaluate(line: &str, n: usize, dir: &Path) -> bool {
    let check_path = dir.join(format!("line{n}.kp"));
    let program = dir.join(format!("line{n}"));
    let check_text = format!("{HEAD}{line}{TAIL}");
    if let Err(e) = fs::write(&check_path, &check_text) {
        eprintln!("error: cannot write '{}': {e}", check_path.display());
        return false;
    }
    let path = check_path.to_string_lossy().into_owned();

    let mut sources = SourceMap::new();
    let (findings, ok) = front_end(&path, true, &mut sources, None);
    if !ok {
        for (severity, d) in &findings {
            show(d, line, *severity);
        }
        remove(&check_path, &program);
        return false;
    }

    let Some(ty) = value_type(&path) else {
        eprintln!("error: could not read the type of this line");
        remove(&check_path, &program);
        return false;
    };
    let run_text = match displayed(&ty, line) {
        Ok(Some(text)) => text,
        Ok(None) => check_text,
        Err(message) => {
            eprintln!("{message}");
            remove(&check_path, &program);
            return false;
        }
    };
    if let Err(e) = fs::write(&check_path, &run_text) {
        eprintln!("error: cannot write '{}': {e}", check_path.display());
        remove(&check_path, &program);
        return false;
    }

    if let Some((available, floor)) = low_memory() {
        eprintln!(
            "error: low memory: {available} MiB available, the floor is {floor} MiB (KELPC_MIN_AVAILABLE_MB); the line was not built"
        );
        remove(&check_path, &program);
        return false;
    }

    let mut freestanding = false;
    let mut emit = resolve::Emit::Binary;
    guard::take_tripped();
    guard::arm();
    let built = build(
        &path,
        &program,
        true,
        OptLevel::DEFAULT,
        false,
        &mut freestanding,
        &mut emit,
        false,
    );
    guard::disarm();
    let tripped = guard::take_tripped();
    if !built {
        if tripped {
            eprintln!(
                "error: low memory: the memory guard stopped the build; the line was not run (KELPC_MIN_AVAILABLE_MB sets the floor)"
            );
        }
        remove(&check_path, &program);
        return false;
    }

    let _ = io::stdout().flush();
    let status = Command::new(&program).stdin(Stdio::null()).status();
    remove(&check_path, &program);
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            match status.code() {
                Some(code) => eprintln!("error: the line stopped with exit status {code}"),
                None => eprintln!("error: the line was stopped by a signal"),
            }
            false
        }
        Err(e) => {
            eprintln!("error: could not run the line: {e}");
            false
        }
    }
}

/// The available memory in MiB and the floor, when available memory is below
/// the floor. `KELPC_MIN_AVAILABLE_MB` sets the floor (`0` turns the check off).
fn low_memory() -> Option<(u64, u64)> {
    let floor = guard::floor_mb(std::env::var("KELPC_MIN_AVAILABLE_MB").ok().as_deref());
    if floor == 0 {
        return None;
    }
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let available = guard::available_kb(&text)? / 1024;
    (available < floor).then_some((available, floor))
}

/// Removes the temporary directories of REPL processes that are no longer
/// running. Only done where `/proc` exists, so the test of "running" means
/// something; the current process's own directory is left alone.
fn sweep_stale(temp: &Path) {
    if !Path::new("/proc/self").exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|n| n.strip_prefix(DIR_PREFIX))
            .and_then(|p| p.parse::<u32>().ok());
        let Some(pid) = pid else {
            continue;
        };
        if pid == std::process::id() || Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        let _ = fs::remove_dir_all(entry.path());
    }
}

/// The type of the typed line, read from the checked `main`. The line is the
/// whole of `main`'s body, so its value is the body's tail.
fn value_type(path: &str) -> Option<Ty> {
    let mut sources = SourceMap::new();
    let loaded = resolve::load(Path::new(path), &mut sources, None, None).ok()?;
    let _ = infer::take_cap_uses();
    let typed = infer::check_program(&loaded.program, &loaded.project).ok()?;
    typed
        .functions
        .iter()
        .find(|f| f.name == "main")
        .map(|f| f.body.ty())
}

/// The program text that shows a value of type `ty`: `None` for a unit line,
/// which prints nothing of its own, and the line wrapped in `print` for a type
/// with a display. Any other type is refused (decision D5).
fn displayed(ty: &Ty, line: &str) -> Result<Option<String>, String> {
    match ty {
        Ty::Unit => Ok(None),
        Ty::I32 | Ty::I64 | Ty::F32 | Ty::F64 | Ty::Bool | Ty::Str | Ty::String => {
            Ok(Some(format!("{HEAD}print({line}\n){TAIL}")))
        }
        other => Err(format!(
            "error: no display for type '{}' yet; stage 1 shows Int, Float, Bool and String",
            other.display()
        )),
    }
}

/// Shows one front-end finding against the typed line. Its offsets are moved
/// from the candidate program to the typed line, and it is named `<repl>`. A
/// finding that points outside the typed line has no location to show.
fn show(d: &Diagnostic, line: &str, severity: Severity) {
    // The primary span is always placed: a span past the typed line (a parse
    // error at the closing brace of the candidate) points at the end of the line.
    let mut shown = Diagnostic::new(d.message.clone(), clamp_span(d.span, line), NAME);
    shown.help = d.help.as_ref().map(|h| {
        if h == NAME_HELP_FROM {
            NAME_HELP.to_string()
        } else {
            h.clone()
        }
    });
    for note in &d.notes {
        if let Some(span) = input_span(note.span, line) {
            shown = shown.with_note(note.message.clone(), span, NAME);
        }
    }
    let mut sources = SourceMap::new();
    sources.insert(NAME, Path::new(NAME), line.to_string());
    sources.report_as(&shown, severity);
}

/// A span in the candidate program, moved to the typed line and clamped to it.
/// A position before the line is the line's start; one after it is its end.
fn clamp_span(span: Span, line: &str) -> Span {
    let lo = HEAD.len();
    let hi = lo + line.len();
    let start = span.start.clamp(lo, hi);
    let end = span.end.clamp(start, hi);
    Span::new(start - lo, end - lo)
}

/// A span in the candidate program, moved to the typed line, or `None` when it
/// lies outside the typed line. Used for notes, which may point at the
/// generated `main` and have no place in the typed line.
fn input_span(span: Span, line: &str) -> Option<Span> {
    let lo = HEAD.len();
    let hi = lo + line.len();
    if span.start < lo || span.start > hi {
        return None;
    }
    let start = span.start - lo;
    let end = span.end.clamp(span.start, hi) - lo;
    Some(Span::new(start, end))
}

fn remove(check_path: &Path, program: &Path) {
    let _ = fs::remove_file(check_path);
    let _ = fs::remove_file(program);
}

/// The per-process directory the candidate programs are built in. It is removed
/// when the command returns.
struct WorkDir {
    path: PathBuf,
}

impl WorkDir {
    fn create() -> io::Result<WorkDir> {
        let path = std::env::temp_dir().join(format!("{DIR_PREFIX}{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)?;
        Ok(WorkDir { path })
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
